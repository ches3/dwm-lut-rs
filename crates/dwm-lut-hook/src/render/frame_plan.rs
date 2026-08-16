use crate::dwmcore::DirtyRect;
use crate::state::{LutAssignment, find_assignment};
use dwm_lut_payload::{ColorMode, MonitorIdentity};

use super::{FrameSize, FrameSkipReason};

pub(crate) const DXGI_FORMAT_R16G16B16A16_FLOAT: u32 = 10;
pub(crate) const DXGI_FORMAT_B8G8R8A8_UNORM: u32 = 87;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum BackBufferFormat {
    Bgra8Unorm,
    Rgba16Float,
}

impl BackBufferFormat {
    pub(super) const fn from_dxgi_format(format: u32) -> Option<Self> {
        match format {
            DXGI_FORMAT_B8G8R8A8_UNORM => Some(Self::Bgra8Unorm),
            DXGI_FORMAT_R16G16B16A16_FLOAT => Some(Self::Rgba16Float),
            _ => None,
        }
    }

    pub(super) const fn dxgi_format(self) -> u32 {
        match self {
            Self::Bgra8Unorm => DXGI_FORMAT_B8G8R8A8_UNORM,
            Self::Rgba16Float => DXGI_FORMAT_R16G16B16A16_FLOAT,
        }
    }

    pub(super) const fn is_hdr(self) -> bool {
        matches!(self, Self::Rgba16Float)
    }

    const fn color_mode(self) -> ColorMode {
        match self {
            Self::Bgra8Unorm => ColorMode::Sdr,
            Self::Rgba16Float => ColorMode::Hdr,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LutSelection {
    pub(super) back_buffer_format: BackBufferFormat,
    pub(super) lut_index: usize,
}

#[derive(Clone, Copy)]
pub(super) enum FrameOperation<'a> {
    Apply {
        lut: LutSelection,
        assignment: &'a LutAssignment,
    },
    Restore,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PreparedFrame {
    pub(super) back_buffer_format: BackBufferFormat,
    pub(super) size: FrameSize,
    pub(super) source_dirty_rects: Vec<DirtyRect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DrawScope {
    DirtyRects,
    FullFrame,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct FramePlan {
    frame: PreparedFrame,
    lut: LutSelection,
    scope: DrawScope,
}

impl FramePlan {
    pub(super) fn source_dirty_rects(&self) -> &[DirtyRect] {
        &self.frame.source_dirty_rects
    }

    pub(super) const fn lut(&self) -> LutSelection {
        self.lut
    }

    pub(super) const fn size(&self) -> FrameSize {
        self.frame.size
    }

    pub(super) const fn scope(&self) -> DrawScope {
        self.scope
    }

    pub(super) const fn present_dirty_rect(&self) -> Option<DirtyRect> {
        match self.scope {
            DrawScope::DirtyRects => None,
            DrawScope::FullFrame => Some(self.frame.size.full_rect()),
        }
    }
}

pub(super) fn prepare_frame(
    dxgi_format: u32,
    size: FrameSize,
    dirty_rects: &[DirtyRect],
) -> Result<PreparedFrame, FrameSkipReason> {
    let format = BackBufferFormat::from_dxgi_format(dxgi_format);

    if size.width == 0 || size.height == 0 {
        return Err(FrameSkipReason::ZeroSize);
    }
    if format.is_none() {
        return Err(FrameSkipReason::UnsupportedFormat);
    }
    let format = format.expect("supported format was checked above");
    let source_dirty_rects = normalize_dirty_rects(dirty_rects, size);
    if source_dirty_rects.is_empty() {
        return Err(FrameSkipReason::NoValidDirtyRects);
    }

    Ok(PreparedFrame {
        back_buffer_format: format,
        size,
        source_dirty_rects,
    })
}

pub(super) fn select_frame_operation(
    assignments: &[LutAssignment],
    monitor_identity: MonitorIdentity,
    format: BackBufferFormat,
) -> FrameOperation<'_> {
    let Some((lut_index, assignment)) =
        find_assignment(assignments, monitor_identity, format.color_mode())
    else {
        return FrameOperation::Restore;
    };
    FrameOperation::Apply {
        lut: LutSelection {
            back_buffer_format: format,
            lut_index,
        },
        assignment,
    }
}

pub(super) fn build_frame_plan(
    frame: PreparedFrame,
    lut: LutSelection,
    previous: Option<LutSelection>,
) -> FramePlan {
    let scope = if previous == Some(lut) {
        DrawScope::DirtyRects
    } else {
        DrawScope::FullFrame
    };
    FramePlan { frame, lut, scope }
}

fn normalize_dirty_rects(dirty_rects: &[DirtyRect], size: FrameSize) -> Vec<DirtyRect> {
    let full_rect;
    // Present treats an empty dirty-rect list as a full-frame update.
    let rects = if dirty_rects.is_empty() {
        full_rect = [size.full_rect()];
        &full_rect[..]
    } else {
        dirty_rects
    };

    rects
        .iter()
        .filter_map(|rect| clamp_rect(*rect, size))
        .collect()
}

fn clamp_rect(rect: DirtyRect, size: FrameSize) -> Option<DirtyRect> {
    let left = rect.left.clamp(0, size.width as i32);
    let top = rect.top.clamp(0, size.height as i32);
    let right = rect.right.clamp(0, size.width as i32);
    let bottom = rect.bottom.clamp(0, size.height as i32);
    (left < right && top < bottom).then_some(DirtyRect {
        left,
        top,
        right,
        bottom,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{LutMetadata, ShaderTexture3D};
    use dwm_lut_payload::{AdapterLuid, MonitorTarget};

    fn identity() -> MonitorIdentity {
        MonitorIdentity {
            adapter_luid: AdapterLuid {
                low_part: 1,
                high_part: 2,
            },
            target_id: 3,
        }
    }

    fn assignment(color_mode: ColorMode) -> LutAssignment {
        LutAssignment {
            target: MonitorTarget {
                identity: identity(),
                color_mode,
            },
            metadata: LutMetadata {
                size: 2,
                domain_min: [0.0; 3],
                domain_max: [1.0; 3],
            },
            texture: ShaderTexture3D {
                width: 2,
                height: 2,
                depth: 2,
                texels: vec![[0.0; 4]; 8],
            },
        }
    }

    fn size() -> FrameSize {
        FrameSize {
            width: 1920,
            height: 1080,
        }
    }

    #[test]
    fn selects_frame_operation_for_back_buffer_format_and_assignment() {
        let assignments = [assignment(ColorMode::Sdr), assignment(ColorMode::Hdr)];
        let FrameOperation::Apply {
            lut: sdr,
            assignment: sdr_assignment,
        } = select_frame_operation(&assignments, identity(), BackBufferFormat::Bgra8Unorm)
        else {
            panic!("SDR assignment should select LUT application");
        };
        let FrameOperation::Apply {
            lut: hdr,
            assignment: hdr_assignment,
        } = select_frame_operation(&assignments, identity(), BackBufferFormat::Rgba16Float)
        else {
            panic!("HDR assignment should select LUT application");
        };

        assert_eq!(sdr.lut_index, 0);
        assert_eq!(hdr.lut_index, 1);
        assert_eq!(sdr_assignment.target.color_mode, ColorMode::Sdr);
        assert_eq!(hdr_assignment.target.color_mode, ColorMode::Hdr);
        assert!(matches!(
            select_frame_operation(&[], identity(), BackBufferFormat::Bgra8Unorm),
            FrameOperation::Restore
        ));
    }

    #[test]
    fn normalizes_dirty_rects_to_frame() {
        let input = prepare_frame(
            DXGI_FORMAT_B8G8R8A8_UNORM,
            size(),
            &[
                DirtyRect {
                    left: -10,
                    top: 20,
                    right: 30,
                    bottom: 40,
                },
                DirtyRect {
                    left: 2000,
                    top: 0,
                    right: 2100,
                    bottom: 100,
                },
            ],
        )
        .unwrap();

        assert_eq!(
            input.source_dirty_rects,
            vec![DirtyRect {
                left: 0,
                top: 20,
                right: 30,
                bottom: 40,
            }]
        );
    }

    #[test]
    fn empty_dirty_rects_mean_full_frame() {
        let input = prepare_frame(DXGI_FORMAT_B8G8R8A8_UNORM, size(), &[]).unwrap();

        assert_eq!(input.source_dirty_rects, vec![size().full_rect()]);
    }

    #[test]
    fn skips_when_no_valid_dirty_rect_remains_after_normalization() {
        let skip = prepare_frame(
            DXGI_FORMAT_B8G8R8A8_UNORM,
            size(),
            &[
                DirtyRect {
                    left: 1920,
                    top: 0,
                    right: 2000,
                    bottom: 100,
                },
                DirtyRect {
                    left: 100,
                    top: 100,
                    right: 100,
                    bottom: 200,
                },
            ],
        )
        .unwrap_err();

        assert_eq!(skip, FrameSkipReason::NoValidDirtyRects);
    }

    #[test]
    fn new_or_changed_lut_requires_full_draw_and_present() {
        let input = prepare_frame(
            DXGI_FORMAT_B8G8R8A8_UNORM,
            size(),
            &[DirtyRect {
                left: 10,
                top: 20,
                right: 30,
                bottom: 40,
            }],
        )
        .unwrap();
        let source_dirty_rects = input.source_dirty_rects.clone();
        let lut = LutSelection {
            back_buffer_format: BackBufferFormat::Bgra8Unorm,
            lut_index: 0,
        };
        let previous_lut = LutSelection {
            back_buffer_format: BackBufferFormat::Bgra8Unorm,
            lut_index: 1,
        };

        let new_back_buffer = build_frame_plan(input.clone(), lut, None);
        let changed_lut = build_frame_plan(input, lut, Some(previous_lut));

        for plan in [new_back_buffer, changed_lut] {
            assert_eq!(plan.scope(), DrawScope::FullFrame);
            assert_eq!(plan.present_dirty_rect(), Some(size().full_rect()));
            assert_eq!(plan.source_dirty_rects(), source_dirty_rects);
        }
    }

    #[test]
    fn unchanged_lut_uses_source_dirty_rects_without_replacing_present_rects() {
        let input = prepare_frame(
            DXGI_FORMAT_B8G8R8A8_UNORM,
            size(),
            &[DirtyRect {
                left: 10,
                top: 20,
                right: 30,
                bottom: 40,
            }],
        )
        .unwrap();
        let lut = LutSelection {
            back_buffer_format: BackBufferFormat::Bgra8Unorm,
            lut_index: 0,
        };

        let plan = build_frame_plan(input, lut, Some(lut));

        assert_eq!(plan.scope(), DrawScope::DirtyRects);
        assert_eq!(plan.present_dirty_rect(), None);
    }

    #[test]
    fn rejects_invalid_size_and_format_independently_of_assignment_selection() {
        let zero_size = prepare_frame(
            DXGI_FORMAT_B8G8R8A8_UNORM,
            FrameSize {
                width: 0,
                height: 1080,
            },
            &[],
        )
        .unwrap_err();
        let unsupported = prepare_frame(999, size(), &[]).unwrap_err();

        assert_eq!(zero_size, FrameSkipReason::ZeroSize);
        assert_eq!(unsupported, FrameSkipReason::UnsupportedFormat);
    }
}
