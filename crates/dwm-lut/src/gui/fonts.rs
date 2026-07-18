use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use eframe::egui::epaint::text::VariationCoords;
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_AXIS_TAG_WEIGHT, DWRITE_FONT_AXIS_VALUE,
    DWRITE_READING_DIRECTION, DWRITE_READING_DIRECTION_LEFT_TO_RIGHT, DWriteCreateFactory,
    IDWriteFactory2, IDWriteFontFace5, IDWriteFontFallback1, IDWriteFontFile,
    IDWriteLocalFontFileLoader, IDWriteNumberSubstitution, IDWriteTextAnalysisSource,
    IDWriteTextAnalysisSource_Impl,
};
use windows::core::{ComObject, Error as WindowsError, Interface, OutRef, PCWSTR, implement};

const BASE_FAMILY: &str = "Segoe UI";
const REGULAR_WEIGHT: f32 = 400.0;
const LOCALE_NAME_CAPACITY: usize = 85;

pub(super) struct SystemFonts {
    fallback: IDWriteFontFallback1,
    locale: Vec<u16>,
    base_family: Vec<u16>,
    loaded: Vec<LoadedFont>,
    loaded_keys: HashMap<FontKey, usize>,
    processed_texts: HashSet<String>,
}

pub(super) struct FontUpdate {
    pub(super) unresolved: Vec<u32>,
}

impl SystemFonts {
    pub(super) fn new(context: &egui::Context) -> Result<Self, FontError> {
        let factory: IDWriteFactory2 = unsafe {
            // DirectWrite returns an owned, reference-counted factory interface.
            DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
        }
        .map_err(|source| FontError::Windows {
            operation: "create DirectWrite factory",
            source,
        })?;
        let fallback = unsafe {
            // The system fallback object is owned by the returned COM interface.
            factory.GetSystemFontFallback()
        }
        .and_then(|fallback| fallback.cast::<IDWriteFontFallback1>())
        .map_err(|source| FontError::Windows {
            operation: "get DirectWrite system font fallback",
            source,
        })?;

        let mut fonts = Self {
            fallback,
            locale: user_locale()?,
            base_family: wide_null(BASE_FAMILY),
            loaded: Vec::new(),
            loaded_keys: HashMap::new(),
            processed_texts: HashSet::new(),
        };
        let resolution = fonts.resolve_text("Aa0", true)?;
        if !resolution.changed {
            return Err(FontError::InvalidSystemFont(
                "DirectWrite did not return the Segoe UI base font".to_string(),
            ));
        }
        fonts.apply(context);
        Ok(fonts)
    }

    pub(super) fn ensure_texts<'a>(
        &mut self,
        context: &egui::Context,
        texts: impl IntoIterator<Item = &'a str>,
    ) -> Result<FontUpdate, FontError> {
        let mut changed = false;
        let mut unresolved = Vec::new();

        for text in texts {
            if text.is_empty() || !self.processed_texts.insert(text.to_string()) {
                continue;
            }
            let needs_fallback = context.fonts_mut(|fonts| {
                !family_supports_text(fonts, egui::FontFamily::Proportional, text)
                    || !family_supports_text(fonts, egui::FontFamily::Monospace, text)
            });
            if !needs_fallback {
                continue;
            }

            let resolution = self.resolve_text(text, false)?;
            changed |= resolution.changed;
            unresolved.extend(resolution.unresolved);
        }

        Ok(self.finish_update(context, changed, unresolved))
    }

    pub(super) fn prepare_input_texts<'a>(
        &mut self,
        context: &egui::Context,
        texts: impl IntoIterator<Item = &'a str>,
    ) -> Result<FontUpdate, FontError> {
        let mut changed = false;
        let mut unresolved = Vec::new();

        for text in texts {
            if text.is_empty() {
                continue;
            }
            let resolution = self.resolve_text(text, false)?;
            changed |= resolution.changed;
            unresolved.extend(resolution.unresolved);
        }

        Ok(self.finish_update(context, changed, unresolved))
    }

    fn finish_update(
        &self,
        context: &egui::Context,
        changed: bool,
        mut unresolved: Vec<u32>,
    ) -> FontUpdate {
        if changed {
            self.apply(context);
        }
        unresolved.sort_unstable();
        unresolved.dedup();
        FontUpdate { unresolved }
    }

    fn resolve_text(&mut self, text: &str, base: bool) -> Result<Resolution, FontError> {
        let source = ComObject::new(TextAnalysisSource::new(text, self.locale.clone()));
        let source: IDWriteTextAnalysisSource = source.into_interface();
        let text_length = text.encode_utf16().count() as u32;
        let axes = [DWRITE_FONT_AXIS_VALUE {
            axisTag: DWRITE_FONT_AXIS_TAG_WEIGHT,
            value: REGULAR_WEIGHT,
        }];
        let mut position = 0;
        let mut changed = false;
        let mut unresolved = Vec::new();

        while position < text_length {
            let mut mapped_length = 0;
            let mut scale = 1.0;
            let mut face = None;
            unsafe {
                // The analysis source and UTF-16 buffers outlive this synchronous call.
                self.fallback.MapCharacters(
                    &source,
                    position,
                    text_length - position,
                    None,
                    PCWSTR(self.base_family.as_ptr()),
                    &axes,
                    &mut mapped_length,
                    &mut scale,
                    &mut face,
                )
            }
            .map_err(|source| FontError::Windows {
                operation: "map text to a system fallback font",
                source,
            })?;
            if mapped_length == 0 {
                return Err(FontError::InvalidSystemFont(
                    "DirectWrite returned an empty mapped range".to_string(),
                ));
            }

            if let Some(face) = face {
                changed |= self.register_face(&face, base)?;
            } else {
                let utf16 = text.encode_utf16().collect::<Vec<_>>();
                let start = position as usize;
                let end = (position + mapped_length) as usize;
                unresolved.extend(
                    String::from_utf16_lossy(&utf16[start..end])
                        .chars()
                        .filter(|character| !is_ignorable(*character))
                        .map(u32::from),
                );
            }
            position += mapped_length;
        }

        Ok(Resolution {
            changed,
            unresolved,
        })
    }

    fn register_face(&mut self, face: &IDWriteFontFace5, base: bool) -> Result<bool, FontError> {
        let (path, index) = font_face_location(face)?;
        let axes = font_face_axes(face)?;
        let key = FontKey {
            path: path.clone(),
            index,
            axes: axes
                .iter()
                .map(|axis| (axis.axisTag.0, axis.value.to_bits()))
                .collect(),
        };
        if let Some(existing) = self.loaded_keys.get(&key).copied() {
            self.loaded[existing].base |= base;
            return Ok(false);
        }

        let bytes = fs::read(&path).map_err(|source| FontError::ReadFont {
            path: path.clone(),
            source,
        })?;
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        let coords = if data.variation_axes().is_empty() {
            VariationCoords::default()
        } else {
            VariationCoords::new(
                axes.iter()
                    .map(|axis| (axis.axisTag.0.to_le_bytes(), axis.value)),
            )
        };
        let data = data.tweak(egui::FontTweak {
            coords,
            ..Default::default()
        });
        let name = format!("system-font-{}", self.loaded.len());
        let loaded_index = self.loaded.len();
        self.loaded.push(LoadedFont {
            name,
            data: Arc::new(data),
            base,
        });
        self.loaded_keys.insert(key, loaded_index);
        Ok(true)
    }

    fn apply(&self, context: &egui::Context) {
        let mut definitions = egui::FontDefinitions::default();
        for font in &self.loaded {
            definitions
                .font_data
                .insert(font.name.clone(), font.data.clone());
        }

        let mut system_order = self
            .loaded
            .iter()
            .filter(|font| font.base)
            .chain(self.loaded.iter().filter(|font| !font.base))
            .map(|font| font.name.clone())
            .collect::<Vec<_>>();
        let proportional = definitions
            .families
            .get_mut(&egui::FontFamily::Proportional)
            .expect("default proportional font family must exist");
        proportional.splice(0..0, system_order.clone());

        let monospace = definitions
            .families
            .get_mut(&egui::FontFamily::Monospace)
            .expect("default monospace font family must exist");
        let insert_at = usize::from(!monospace.is_empty());
        monospace.splice(insert_at..insert_at, system_order.drain(..));
        context.set_fonts(definitions);
        context.request_repaint();
    }
}

struct Resolution {
    changed: bool,
    unresolved: Vec<u32>,
}

struct LoadedFont {
    name: String,
    data: Arc<egui::FontData>,
    base: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FontKey {
    path: PathBuf,
    index: u32,
    axes: Vec<(u32, u32)>,
}

#[implement(IDWriteTextAnalysisSource)]
struct TextAnalysisSource {
    text: Vec<u16>,
    locale: Vec<u16>,
}

impl TextAnalysisSource {
    fn new(text: &str, locale: Vec<u16>) -> Self {
        Self {
            text: text.encode_utf16().collect(),
            locale,
        }
    }
}

impl IDWriteTextAnalysisSource_Impl for TextAnalysisSource_Impl {
    fn GetTextAtPosition(
        &self,
        text_position: u32,
        text_string: *mut *mut u16,
        text_length: *mut u32,
    ) -> windows::core::Result<()> {
        let position = text_position as usize;
        unsafe {
            // DirectWrite only retains these pointers for the duration of MapCharacters.
            if position >= self.text.len() {
                text_string.write(std::ptr::null_mut());
                text_length.write(0);
            } else {
                text_string.write(self.text.as_ptr().add(position).cast_mut());
                text_length.write((self.text.len() - position) as u32);
            }
        }
        Ok(())
    }

    fn GetTextBeforePosition(
        &self,
        text_position: u32,
        text_string: *mut *mut u16,
        text_length: *mut u32,
    ) -> windows::core::Result<()> {
        let position = (text_position as usize).min(self.text.len());
        unsafe {
            // The returned prefix points into the analysis source's stable UTF-16 buffer.
            if position == 0 {
                text_string.write(std::ptr::null_mut());
                text_length.write(0);
            } else {
                text_string.write(self.text.as_ptr().cast_mut());
                text_length.write(position as u32);
            }
        }
        Ok(())
    }

    fn GetParagraphReadingDirection(&self) -> DWRITE_READING_DIRECTION {
        DWRITE_READING_DIRECTION_LEFT_TO_RIGHT
    }

    fn GetLocaleName(
        &self,
        text_position: u32,
        text_length: *mut u32,
        locale_name: *mut *mut u16,
    ) -> windows::core::Result<()> {
        let position = (text_position as usize).min(self.text.len());
        unsafe {
            // The locale buffer is NUL-terminated and owned by this analysis source.
            text_length.write((self.text.len() - position) as u32);
            locale_name.write(self.locale.as_ptr().cast_mut());
        }
        Ok(())
    }

    fn GetNumberSubstitution(
        &self,
        text_position: u32,
        text_length: *mut u32,
        number_substitution: OutRef<IDWriteNumberSubstitution>,
    ) -> windows::core::Result<()> {
        let position = (text_position as usize).min(self.text.len());
        unsafe {
            // Number substitution is not needed to select a fallback font.
            text_length.write((self.text.len() - position) as u32);
        }
        _ = number_substitution.write(None);
        Ok(())
    }
}

fn user_locale() -> Result<Vec<u16>, FontError> {
    let mut locale = vec![0; LOCALE_NAME_CAPACITY];
    let length = unsafe {
        // The buffer capacity follows LOCALE_NAME_MAX_LENGTH from the Windows SDK.
        GetUserDefaultLocaleName(&mut locale)
    };
    if length == 0 {
        return Err(FontError::Windows {
            operation: "get the user locale",
            source: WindowsError::from_thread(),
        });
    }
    locale.truncate(length as usize);
    Ok(locale)
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn font_face_location(face: &IDWriteFontFace5) -> Result<(PathBuf, u32), FontError> {
    let mut file_count = 0;
    unsafe {
        // The first call only queries the number of backing files.
        face.GetFiles(&mut file_count, None)
    }
    .map_err(|source| FontError::Windows {
        operation: "query DirectWrite font files",
        source,
    })?;
    if file_count != 1 {
        return Err(FontError::InvalidSystemFont(format!(
            "expected one local font file, got {file_count}"
        )));
    }

    let mut files = vec![None; file_count as usize];
    unsafe {
        // The allocated array has exactly the size returned by DirectWrite.
        face.GetFiles(&mut file_count, Some(files.as_mut_ptr()))
    }
    .map_err(|source| FontError::Windows {
        operation: "get DirectWrite font files",
        source,
    })?;
    let file = files
        .pop()
        .flatten()
        .ok_or_else(|| FontError::InvalidSystemFont("font file was null".to_string()))?;
    let path = local_font_path(&file)?;
    let index = unsafe {
        // The face index identifies the selected font inside TTC collections.
        face.GetIndex()
    };
    Ok((path, index))
}

fn local_font_path(file: &IDWriteFontFile) -> Result<PathBuf, FontError> {
    let loader: IDWriteLocalFontFileLoader = unsafe {
        // System fallback fonts are expected to use the local font file loader.
        file.GetLoader()
    }
    .and_then(|loader| loader.cast())
    .map_err(|source| FontError::Windows {
        operation: "get local DirectWrite font loader",
        source,
    })?;
    let mut key = std::ptr::null_mut();
    let mut key_size = 0;
    unsafe {
        // The reference key remains owned by the font file while it is in scope.
        file.GetReferenceKey(&mut key, &mut key_size)
    }
    .map_err(|source| FontError::Windows {
        operation: "get DirectWrite font file key",
        source,
    })?;
    let path_length = unsafe {
        // The key pointer and size originate from this loader's font file.
        loader.GetFilePathLengthFromKey(key, key_size)
    }
    .map_err(|source| FontError::Windows {
        operation: "query DirectWrite font path length",
        source,
    })?;
    let mut path = vec![0; path_length as usize + 1];
    unsafe {
        // The buffer includes space for the terminating NUL expected by DirectWrite.
        loader.GetFilePathFromKey(key, key_size, &mut path)
    }
    .map_err(|source| FontError::Windows {
        operation: "get DirectWrite font path",
        source,
    })?;
    path.truncate(path_length as usize);
    Ok(PathBuf::from(OsString::from_wide(&path)))
}

fn font_face_axes(face: &IDWriteFontFace5) -> Result<Vec<DWRITE_FONT_AXIS_VALUE>, FontError> {
    let count = unsafe {
        // The face owns its immutable variation axis list.
        face.GetFontAxisValueCount()
    };
    let mut axes = vec![DWRITE_FONT_AXIS_VALUE::default(); count as usize];
    if !axes.is_empty() {
        unsafe {
            // The output slice has the exact axis count reported by the face.
            face.GetFontAxisValues(&mut axes)
        }
        .map_err(|source| FontError::Windows {
            operation: "get DirectWrite font variation axes",
            source,
        })?;
    }
    Ok(axes)
}

fn is_ignorable(character: char) -> bool {
    character.is_control()
        || character.is_whitespace()
        || matches!(
            character as u32,
            0x00ad
                | 0x034f
                | 0x061c
                | 0x180e
                | 0x200b..=0x200f
                | 0x202a..=0x202e
                | 0x2060..=0x206f
                | 0xfe00..=0xfe0f
                | 0xfeff
                | 0xe0100..=0xe01ef
        )
}

fn family_supports_text(
    fonts: &mut egui::epaint::text::FontsView<'_>,
    family: egui::FontFamily,
    text: &str,
) -> bool {
    let mut font = fonts.fonts.font(&family);
    let characters = font.characters();
    text.chars()
        .all(|character| is_ignorable(character) || characters.contains_key(&character))
}

#[derive(Debug)]
pub(super) enum FontError {
    Windows {
        operation: &'static str,
        source: WindowsError,
    },
    ReadFont {
        path: PathBuf,
        source: std::io::Error,
    },
    InvalidSystemFont(String),
}

impl fmt::Display for FontError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Windows { operation, source } => write!(formatter, "{operation}: {source}"),
            Self::ReadFont { path, source } => {
                write!(formatter, "read system font {}: {source}", path.display())
            }
            Self::InvalidSystemFont(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for FontError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Windows { source, .. } => Some(source),
            Self::ReadFont { source, .. } => Some(source),
            Self::InvalidSystemFont(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepares_installed_fonts_for_common_scripts_before_next_pass() {
        let context = egui::Context::default();
        let mut fonts = SystemFonts::new(&context).unwrap();
        let corpus = "Latin Ελληνικά Кириллица العربية עברית हिन्दी ไทย 日本語 中文 한국어 ∑ 🧪";

        let update = fonts.prepare_input_texts(&context, [corpus]).unwrap();
        _ = context.run_ui(egui::RawInput::default(), |_| {});

        assert!(
            update.unresolved.is_empty(),
            "unresolved code points: {:?}",
            update.unresolved
        );
        context.fonts_mut(|loaded| {
            let proportional_missing = {
                let mut font = loaded.fonts.font(&egui::FontFamily::Proportional);
                let characters = font.characters();
                corpus
                    .chars()
                    .filter(|character| {
                        !is_ignorable(*character) && !characters.contains_key(character)
                    })
                    .map(|character| format!("U+{:04X}", character as u32))
                    .collect::<Vec<_>>()
            };
            let monospace_missing = {
                let mut font = loaded.fonts.font(&egui::FontFamily::Monospace);
                let characters = font.characters();
                corpus
                    .chars()
                    .filter(|character| {
                        !is_ignorable(*character) && !characters.contains_key(character)
                    })
                    .map(|character| format!("U+{:04X}", character as u32))
                    .collect::<Vec<_>>()
            };
            assert!(
                proportional_missing.is_empty(),
                "proportional missing: {proportional_missing:?}; loaded: {:?}",
                fonts.loaded_keys.keys().collect::<Vec<_>>()
            );
            assert!(
                monospace_missing.is_empty(),
                "monospace missing: {monospace_missing:?}"
            );
        });
    }
}
