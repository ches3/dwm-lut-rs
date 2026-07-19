use std::path::{Path, PathBuf};

use crate::config::{ConfigDocument, load_config_document, save_config_document};
use crate::paths::default_config_path;

use super::super::error::GuiError;

pub(crate) enum ConfigState {
    Ready(ConfigEditor),
    LoadFailed {
        path: Option<PathBuf>,
        error: GuiError,
    },
}

pub(crate) struct ConfigEditor {
    pub(crate) path: PathBuf,
    pub(crate) document: ConfigDocument,
    pub(crate) selected_profile: String,
}

impl ConfigState {
    pub(crate) fn load_default() -> Self {
        match default_config_path() {
            Ok(path) => Self::load(path),
            Err(error) => Self::LoadFailed {
                path: None,
                error: error.into(),
            },
        }
    }

    pub(crate) fn load(path: PathBuf) -> Self {
        Self::load_selecting(path, None)
    }

    pub(crate) fn load_selecting(path: PathBuf, selected_profile: Option<&str>) -> Self {
        match load_config_document(&path) {
            Ok(document) => {
                if !path.exists()
                    && let Err(error) = save_config_document(&path, &document)
                {
                    return Self::LoadFailed {
                        path: Some(path),
                        error: error.into(),
                    };
                }
                let selected_profile = selected_profile
                    .and_then(|profile| document.profile_key(profile))
                    .unwrap_or_else(|| document.default_profile.clone());
                Self::Ready(ConfigEditor {
                    path,
                    document,
                    selected_profile,
                })
            }
            Err(error) => Self::LoadFailed {
                path: Some(path),
                error: error.into(),
            },
        }
    }

    pub(crate) fn reload(&self) -> Self {
        match self {
            Self::Ready(editor) => {
                Self::load_selecting(editor.path.clone(), Some(editor.selected_profile.as_str()))
            }
            Self::LoadFailed {
                path: Some(path), ..
            } => Self::load(path.clone()),
            Self::LoadFailed { path: None, .. } => Self::load_default(),
        }
    }

    pub(crate) fn retry(&self) -> Self {
        match self {
            Self::Ready(editor) => Self::load(editor.path.clone()),
            Self::LoadFailed {
                path: Some(path), ..
            } => Self::load(path.clone()),
            Self::LoadFailed { path: None, .. } => Self::load_default(),
        }
    }

    pub(crate) fn load_error(&self) -> Option<&GuiError> {
        match self {
            Self::Ready(_) => None,
            Self::LoadFailed { error, .. } => Some(error),
        }
    }

    pub(crate) fn editor(&self) -> Option<&ConfigEditor> {
        match self {
            Self::Ready(editor) => Some(editor),
            Self::LoadFailed { .. } => None,
        }
    }

    pub(crate) fn editor_mut(&mut self) -> Option<&mut ConfigEditor> {
        match self {
            Self::Ready(editor) => Some(editor),
            Self::LoadFailed { .. } => None,
        }
    }
}

pub(crate) fn edit_and_save_config<T, E>(
    path: &Path,
    mut config: ConfigDocument,
    edit: impl FnOnce(&mut ConfigDocument) -> Result<T, E>,
) -> Result<(ConfigDocument, T), GuiError>
where
    E: Into<GuiError>,
{
    let result = edit(&mut config).map_err(Into::into)?;
    save_config_document(path, &config)?;
    Ok((config, result))
}
