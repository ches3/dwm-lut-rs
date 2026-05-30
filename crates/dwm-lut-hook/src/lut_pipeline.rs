use std::fmt;
use std::sync::LazyLock;

use dwm_lut_config::{
    ColorMode, ConfigError, LutAssignment, LutCube, LutManifest, MonitorIdentity, parse_cube,
};

use crate::blue_noise::{blue_noise_threshold, render_blue_noise_hlsl};

pub const DXGI_FORMAT_R16G16B16A16_FLOAT: u32 = 10;
pub const DXGI_FORMAT_B8G8R8A8_UNORM: u32 = 87;
const SDR_DITHER_GAMMA: f32 = 2.2;
const LUT_PIPELINE_SHADER_TEMPLATE: &str = include_str!("../shaders/lut_pipeline.hlsl");
static LUT_PIPELINE_SHADER_SOURCE: LazyLock<String> =
    LazyLock::new(build_lut_pipeline_shader_source);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackBufferFormat {
    Bgra8Unorm,
    Rgba16Float,
}

impl BackBufferFormat {
    pub const fn from_dxgi_format(format: u32) -> Option<Self> {
        match format {
            DXGI_FORMAT_B8G8R8A8_UNORM => Some(Self::Bgra8Unorm),
            DXGI_FORMAT_R16G16B16A16_FLOAT => Some(Self::Rgba16Float),
            _ => None,
        }
    }

    pub const fn is_hdr(self) -> bool {
        matches!(self, Self::Rgba16Float)
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipBox {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirtyRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShaderConstants {
    pub lut_size: u32,
    pub hdr: u32,
    pub domain_min: [f32; 4],
    pub domain_max: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShaderConstantsCBuffer {
    pub lut_size: u32,
    pub hdr: u32,
    pub padding: [f32; 2],
    pub domain_min: [f32; 4],
    pub domain_max: [f32; 4],
}

impl ShaderConstants {
    pub const fn to_cbuffer(self) -> ShaderConstantsCBuffer {
        ShaderConstantsCBuffer {
            lut_size: self.lut_size,
            hdr: self.hdr,
            padding: [0.0, 0.0],
            domain_min: self.domain_min,
            domain_max: self.domain_max,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShaderTexture3D {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub texels: Vec<[f32; 4]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedLut {
    pub assignment: LutAssignment,
    pub metadata: LutMetadata,
    pub texture: ShaderTexture3D,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LutMetadata {
    pub size: u32,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LutShaderProgram {
    pub source: &'static str,
    pub vertex_entry: &'static str,
    pub pixel_entry: &'static str,
    pub vertex_profile: &'static str,
    pub pixel_profile: &'static str,
}

impl LutShaderProgram {
    pub fn embedded() -> Self {
        Self {
            source: LUT_PIPELINE_SHADER_SOURCE.as_str(),
            vertex_entry: "VS",
            pixel_entry: "PS",
            vertex_profile: "vs_5_0",
            pixel_profile: "ps_5_0",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LutPipeline {
    pub luts: Vec<LoadedLut>,
    pub shader: LutShaderProgram,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LutPipelineSummary {
    pub lut_count: usize,
    pub shader_profile: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LutRenderPlan {
    pub format: BackBufferFormat,
    pub clip_box: ClipBox,
    pub dirty_rects: Vec<DirtyRect>,
    pub lut_index: usize,
    pub shader_constants: ShaderConstants,
}

#[derive(Debug)]
pub enum LutPipelineError {
    NoAssignments,
    Config(ConfigError),
}

impl fmt::Display for LutPipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAssignments => write!(f, "manifest does not contain any LUT assignments"),
            Self::Config(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for LutPipelineError {}

impl From<ConfigError> for LutPipelineError {
    fn from(value: ConfigError) -> Self {
        Self::Config(value)
    }
}

impl LutPipeline {
    pub fn load(manifest: &LutManifest) -> Result<Self, LutPipelineError> {
        if manifest.assignments.is_empty() {
            return Err(LutPipelineError::NoAssignments);
        }

        let mut luts = Vec::with_capacity(manifest.assignments.len());
        for assignment in &manifest.assignments {
            let cube = parse_cube(&assignment.lut_path)?;

            luts.push(LoadedLut {
                assignment: assignment.clone(),
                metadata: LutMetadata {
                    size: cube.size,
                    domain_min: cube.domain_min,
                    domain_max: cube.domain_max,
                },
                texture: cube_to_texture(&cube),
            });
        }

        Ok(Self {
            luts,
            shader: LutShaderProgram::embedded(),
        })
    }

    pub fn summary(&self) -> LutPipelineSummary {
        LutPipelineSummary {
            lut_count: self.luts.len(),
            shader_profile: self.shader.pixel_profile,
        }
    }

    pub fn select_lut_index_for_monitor_identity(
        &self,
        identity: MonitorIdentity,
        format: BackBufferFormat,
    ) -> Option<usize> {
        let color_mode = match format {
            BackBufferFormat::Bgra8Unorm => ColorMode::Sdr,
            BackBufferFormat::Rgba16Float => ColorMode::Hdr,
        };

        self.luts.iter().position(|lut| {
            let target = &lut.assignment.target;
            target.identity == identity && target.color_mode == color_mode
        })
    }

    pub fn build_present_plan_for_monitor_identity(
        &self,
        identity: MonitorIdentity,
        clip_box: ClipBox,
        dxgi_format: u32,
        dirty_rects: &[DirtyRect],
    ) -> Option<LutRenderPlan> {
        let format = BackBufferFormat::from_dxgi_format(dxgi_format)?;
        let lut_index = self.select_lut_index_for_monitor_identity(identity, format)?;
        self.build_present_plan_for_index(clip_box, format, dirty_rects, lut_index)
    }

    pub fn build_present_plan_for_lut_index(
        &self,
        clip_box: ClipBox,
        dxgi_format: u32,
        dirty_rects: &[DirtyRect],
        lut_index: usize,
    ) -> Option<LutRenderPlan> {
        let format = BackBufferFormat::from_dxgi_format(dxgi_format)?;
        let lut = self.luts.get(lut_index)?;
        let color_mode = match format {
            BackBufferFormat::Bgra8Unorm => ColorMode::Sdr,
            BackBufferFormat::Rgba16Float => ColorMode::Hdr,
        };
        (lut.assignment.target.color_mode == color_mode)
            .then(|| self.build_present_plan_for_index(clip_box, format, dirty_rects, lut_index))
            .flatten()
    }

    fn build_present_plan_for_index(
        &self,
        clip_box: ClipBox,
        format: BackBufferFormat,
        dirty_rects: &[DirtyRect],
        lut_index: usize,
    ) -> Option<LutRenderPlan> {
        let lut = self.luts.get(lut_index)?;

        Some(LutRenderPlan {
            format,
            clip_box,
            dirty_rects: dirty_rects.to_vec(),
            lut_index,
            shader_constants: ShaderConstants {
                lut_size: lut.metadata.size,
                hdr: u32::from(format.is_hdr()),
                domain_min: extend_domain(lut.metadata.domain_min),
                domain_max: extend_domain(lut.metadata.domain_max),
            },
        })
    }
}

pub fn cube_to_texture(cube: &LutCube) -> ShaderTexture3D {
    let texels = cube
        .values
        .iter()
        .map(|value| [value[0], value[1], value[2], 1.0])
        .collect();

    ShaderTexture3D {
        width: cube.size,
        height: cube.size,
        depth: cube.size,
        texels,
    }
}

pub fn tetrahedral_interpolation(cube: &LutCube, rgb: [f32; 3]) -> [f32; 3] {
    let normalized = normalize_sample(cube, rgb);
    let scale = (cube.size - 1) as f32;
    let index = [
        normalized[0] * scale,
        normalized[1] * scale,
        normalized[2] * scale,
    ];
    let base = [
        index[0].floor() as u32,
        index[1].floor() as u32,
        index[2].floor() as u32,
    ];
    let frac = [
        index[0] - base[0] as f32,
        index[1] - base[1] as f32,
        index[2] - base[2] as f32,
    ];

    let c000 = sample_cube(cube, base[0], base[1], base[2]);
    let c100 = sample_cube(cube, base[0] + 1, base[1], base[2]);
    let c010 = sample_cube(cube, base[0], base[1] + 1, base[2]);
    let c001 = sample_cube(cube, base[0], base[1], base[2] + 1);
    let c110 = sample_cube(cube, base[0] + 1, base[1] + 1, base[2]);
    let c101 = sample_cube(cube, base[0] + 1, base[1], base[2] + 1);
    let c011 = sample_cube(cube, base[0], base[1] + 1, base[2] + 1);
    let c111 = sample_cube(cube, base[0] + 1, base[1] + 1, base[2] + 1);

    let (x, y, z) = (frac[0], frac[1], frac[2]);
    if x >= y {
        if y >= z {
            combine_tetrahedral(c000, c100, c110, c111, x, y, z)
        } else if x >= z {
            combine_tetrahedral(c000, c100, c101, c111, x, z, y)
        } else {
            combine_tetrahedral(c000, c001, c101, c111, z, x, y)
        }
    } else if z >= y {
        combine_tetrahedral(c000, c001, c011, c111, z, y, x)
    } else if z >= x {
        combine_tetrahedral(c000, c010, c011, c111, y, z, x)
    } else {
        combine_tetrahedral(c000, c010, c110, c111, y, x, z)
    }
}

pub fn apply_sdr_dither(rgb: [f32; 3], pixel_x: usize, pixel_y: usize) -> [f32; 3] {
    let threshold = blue_noise_threshold(pixel_x, pixel_y);
    let low = quantize_to_8bit_floor(rgb);
    let high = [
        (low[0] + (1.0 / 255.0)).min(1.0),
        (low[1] + (1.0 / 255.0)).min(1.0),
        (low[2] + (1.0 / 255.0)).min(1.0),
    ];

    let rgb_linear = rgb.map(|value| value.clamp(0.0, 1.0).powf(SDR_DITHER_GAMMA));
    let low_linear = low.map(|value| value.powf(SDR_DITHER_GAMMA));
    let high_linear = high.map(|value| value.powf(SDR_DITHER_GAMMA));

    [
        if rgb_linear[0] > lerp(low_linear[0], high_linear[0], threshold) {
            high[0]
        } else {
            low[0]
        },
        if rgb_linear[1] > lerp(low_linear[1], high_linear[1], threshold) {
            high[1]
        } else {
            low[1]
        },
        if rgb_linear[2] > lerp(low_linear[2], high_linear[2], threshold) {
            high[2]
        } else {
            low[2]
        },
    ]
}

pub fn scrgb_to_pq(rgb: [f32; 3]) -> [f32; 3] {
    linear_bt2100_to_pq(multiply_matrix(SCRGB_TO_BT2100, rgb))
}

pub fn pq_to_scrgb(rgb: [f32; 3]) -> [f32; 3] {
    multiply_matrix(BT2100_TO_SCRGB, pq_to_linear_bt2100(rgb))
}

fn build_lut_pipeline_shader_source() -> String {
    LUT_PIPELINE_SHADER_TEMPLATE.replace("__BLUE_NOISE_64X64__", &render_blue_noise_hlsl())
}

fn normalize_sample(cube: &LutCube, rgb: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|index| {
        let min = cube.domain_min[index];
        let max = cube.domain_max[index];
        if (max - min).abs() <= f32::EPSILON {
            return 0.0;
        }

        ((rgb[index] - min) / (max - min)).clamp(0.0, 1.0)
    })
}

fn extend_domain(domain: [f32; 3]) -> [f32; 4] {
    [domain[0], domain[1], domain[2], 0.0]
}

fn sample_cube(cube: &LutCube, red: u32, green: u32, blue: u32) -> [f32; 3] {
    let max = cube.size.saturating_sub(1);
    let red = red.min(max) as usize;
    let green = green.min(max) as usize;
    let blue = blue.min(max) as usize;
    let size = cube.size as usize;
    cube.values[(blue * size * size) + (green * size) + red]
}

fn combine_tetrahedral(
    c0: [f32; 3],
    c1: [f32; 3],
    c2: [f32; 3],
    c3: [f32; 3],
    a: f32,
    b: f32,
    c: f32,
) -> [f32; 3] {
    [
        c0[0] + a * (c1[0] - c0[0]) + b * (c2[0] - c1[0]) + c * (c3[0] - c2[0]),
        c0[1] + a * (c1[1] - c0[1]) + b * (c2[1] - c1[1]) + c * (c3[1] - c2[1]),
        c0[2] + a * (c1[2] - c0[2]) + b * (c2[2] - c1[2]) + c * (c3[2] - c2[2]),
    ]
}

fn quantize_to_8bit_floor(rgb: [f32; 3]) -> [f32; 3] {
    [
        ((rgb[0].clamp(0.0, 1.0) * 255.0).floor()) / 255.0,
        ((rgb[1].clamp(0.0, 1.0) * 255.0).floor()) / 255.0,
        ((rgb[2].clamp(0.0, 1.0) * 255.0).floor()) / 255.0,
    ]
}

fn lerp(a: f32, b: f32, factor: f32) -> f32 {
    a + (b - a) * factor
}

fn linear_to_pq(linear: f32) -> f32 {
    const M1: f32 = 2610.0 / 16384.0;
    const M2: f32 = 2523.0 / 32.0;
    const C1: f32 = 3424.0 / 4096.0;
    const C2: f32 = 2413.0 / 128.0;
    const C3: f32 = 2392.0 / 128.0;

    let powered = linear.clamp(0.0, 1.0).powf(M1);
    ((C1 + C2 * powered) / (1.0 + C3 * powered)).powf(M2)
}

fn pq_to_linear(pq: f32) -> f32 {
    const M1: f32 = 2610.0 / 16384.0;
    const M2: f32 = 2523.0 / 32.0;
    const C1: f32 = 3424.0 / 4096.0;
    const C2: f32 = 2413.0 / 128.0;
    const C3: f32 = 2392.0 / 128.0;

    let powered = pq.clamp(0.0, 1.0).powf(1.0 / M2);
    let numerator = (powered - C1).max(0.0);
    let denominator = C2 - C3 * powered;
    if denominator <= 0.0 {
        return 1.0;
    }

    (numerator / denominator).powf(1.0 / M1)
}

const SCRGB_TO_BT2100: [[f32; 3]; 3] = [
    [
        2939026994.0 / 585553224375.0,
        9255011753.0 / 3513319346250.0,
        173911579.0 / 501902763750.0,
    ],
    [
        76515593.0 / 138420033750.0,
        6109575001.0 / 830520202500.0,
        75493061.0 / 830520202500.0,
    ],
    [
        12225392.0 / 93230009375.0,
        1772384008.0 / 2517210253125.0,
        18035212433.0 / 2517210253125.0,
    ],
];
const BT2100_TO_SCRGB: [[f32; 3]; 3] = [
    [
        348196442125.0 / 1677558947.0,
        -123225331250.0 / 1677558947.0,
        -15276242500.0 / 1677558947.0,
    ],
    [
        -579752563250.0 / 37238079773.0,
        5273377093000.0 / 37238079773.0,
        -38864558125.0 / 37238079773.0,
    ],
    [
        -12183628000.0 / 5369968309.0,
        -472592308000.0 / 37589778163.0,
        5256599974375.0 / 37589778163.0,
    ],
];

fn multiply_matrix(matrix: [[f32; 3]; 3], value: [f32; 3]) -> [f32; 3] {
    [
        matrix[0][0] * value[0] + matrix[0][1] * value[1] + matrix[0][2] * value[2],
        matrix[1][0] * value[0] + matrix[1][1] * value[1] + matrix[1][2] * value[2],
        matrix[2][0] * value[0] + matrix[2][1] * value[1] + matrix[2][2] * value[2],
    ]
}

fn linear_bt2100_to_pq(rgb: [f32; 3]) -> [f32; 3] {
    rgb.map(linear_to_pq)
}

fn pq_to_linear_bt2100(rgb: [f32; 3]) -> [f32; 3] {
    rgb.map(pq_to_linear)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::mem::size_of;
    use std::path::PathBuf;
    use std::ptr::addr_of;
    use std::time::{SystemTime, UNIX_EPOCH};

    use dwm_lut_config::{
        AdapterLuid, ColorMode, LutAssignment, LutCube, LutManifest, MonitorIdentity, MonitorTarget,
    };

    use super::{
        BackBufferFormat, ClipBox, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT,
        DirtyRect, LutPipeline, ShaderConstants, ShaderConstantsCBuffer, apply_sdr_dither,
        normalize_sample, pq_to_scrgb, scrgb_to_pq, tetrahedral_interpolation,
    };

    fn identity_cube() -> LutCube {
        LutCube {
            size: 2,
            domain_min: [0.0, 0.0, 0.0],
            domain_max: [1.0, 1.0, 1.0],
            values: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
                [0.0, 1.0, 1.0],
                [1.0, 1.0, 1.0],
            ],
        }
    }

    fn write_test_cube() -> PathBuf {
        write_test_cube_contents(
            "LUT_3D_SIZE 2\n\
0.0 0.0 0.0\n\
1.0 0.0 0.0\n\
0.0 1.0 0.0\n\
1.0 1.0 0.0\n\
0.0 0.0 1.0\n\
1.0 0.0 1.0\n\
0.0 1.0 1.0\n\
1.0 1.0 1.0\n",
        )
    }

    fn write_test_cube_contents(contents: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("dwm-lut-pipeline-{unique}.cube"));
        fs::write(&path, contents).expect("cube file should be written");
        path
    }

    #[test]
    fn tetrahedral_interpolation_preserves_identity_cube() {
        let result = tetrahedral_interpolation(&identity_cube(), [0.25, 0.5, 0.75]);

        assert!((result[0] - 0.25).abs() < 1e-6);
        assert!((result[1] - 0.5).abs() < 1e-6);
        assert!((result[2] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn sdr_dither_quantizes_to_adjacent_8bit_steps() {
        let result = apply_sdr_dither([0.5, 0.5, 0.5], 1, 2);

        for channel in result {
            let quantized = channel * 255.0;
            assert!((quantized.round() - quantized).abs() < 1e-6);
        }
    }

    #[test]
    fn sdr_dither_is_deterministic_for_same_pixel() {
        let first = apply_sdr_dither([0.5, 0.5, 0.5], 3, 4);
        let second = apply_sdr_dither([0.5, 0.5, 0.5], 3, 4);

        assert_eq!(first, second);
    }

    #[test]
    fn normalize_sample_supports_descending_domain_ranges() {
        let cube = LutCube {
            size: 2,
            domain_min: [1.0, 0.0, 0.0],
            domain_max: [0.0, 1.0, 1.0],
            values: identity_cube().values,
        };

        let normalized = normalize_sample(&cube, [0.75, 0.5, 0.5]);

        assert_eq!(normalized, [0.25, 0.5, 0.5]);
    }

    #[test]
    fn normalize_sample_maps_zero_width_domain_axis_to_zero() {
        let cube = LutCube {
            size: 2,
            domain_min: [0.5, 0.0, 0.0],
            domain_max: [0.5, 1.0, 1.0],
            values: identity_cube().values,
        };

        let normalized = normalize_sample(&cube, [0.75, 0.5, 0.25]);

        assert_eq!(normalized, [0.0, 0.5, 0.25]);
    }

    #[test]
    fn present_plan_selects_sdr_lut_by_runtime_identity() {
        let cube_path = write_test_cube();
        let identity = MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e02,
            },
            target_id: 4357,
        };
        let mut manifest = LutManifest::empty();
        manifest.add(LutAssignment {
            target: MonitorTarget {
                identity,
                color_mode: ColorMode::Sdr,
            },
            lut_path: cube_path.clone(),
        });

        let runtime = LutPipeline::load(&manifest).expect("runtime should load");
        let plan = runtime
            .build_present_plan_for_monitor_identity(
                identity,
                ClipBox {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                DXGI_FORMAT_B8G8R8A8_UNORM,
                &[DirtyRect {
                    left: 0,
                    top: 0,
                    right: 64,
                    bottom: 64,
                }],
            )
            .expect("plan should exist");

        assert_eq!(plan.format, BackBufferFormat::Bgra8Unorm);
        assert_eq!(plan.shader_constants.lut_size, 2);
        assert_eq!(plan.shader_constants.hdr, 0);
        assert_eq!(plan.shader_constants.domain_min, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(plan.shader_constants.domain_max, [1.0, 1.0, 1.0, 0.0]);
        assert_eq!(plan.dirty_rects.len(), 1);

        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn present_plan_selects_hdr_lut_for_rgba16_float() {
        let cube_path = write_test_cube();
        let identity = MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e02,
            },
            target_id: 4357,
        };
        let mut manifest = LutManifest::empty();
        manifest.add(LutAssignment {
            target: MonitorTarget {
                identity,
                color_mode: ColorMode::Hdr,
            },
            lut_path: cube_path.clone(),
        });

        let runtime = LutPipeline::load(&manifest).expect("runtime should load");
        let plan = runtime.build_present_plan_for_monitor_identity(
            identity,
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            DXGI_FORMAT_R16G16B16A16_FLOAT,
            &[],
        );

        let plan = plan.expect("HDR plan should exist");
        assert_eq!(plan.format, BackBufferFormat::Rgba16Float);
        assert_eq!(plan.shader_constants.hdr, 1);

        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn present_plan_selects_monitor_by_runtime_identity() {
        let cube_path_a = write_test_cube();
        let cube_path_b = write_test_cube();
        let identity_a = MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e02,
            },
            target_id: 4355,
        };
        let identity_b = MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e02,
            },
            target_id: 4357,
        };
        let mut manifest = LutManifest::empty();
        manifest.add(LutAssignment {
            target: MonitorTarget {
                identity: identity_a,
                color_mode: ColorMode::Sdr,
            },
            lut_path: cube_path_a.clone(),
        });
        manifest.add(LutAssignment {
            target: MonitorTarget {
                identity: identity_b,
                color_mode: ColorMode::Sdr,
            },
            lut_path: cube_path_b.clone(),
        });

        let runtime = LutPipeline::load(&manifest).expect("runtime should load");
        assert_eq!(
            runtime
                .build_present_plan_for_monitor_identity(
                    identity_b,
                    ClipBox {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    },
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                    &[],
                )
                .expect("identity should select a plan")
                .lut_index,
            1
        );

        let _ = fs::remove_file(cube_path_a);
        let _ = fs::remove_file(cube_path_b);
    }

    #[test]
    fn hdr_pq_conversion_uses_bt2100_before_pq() {
        let values = [0.25, 0.5, 1.0];
        let pq = scrgb_to_pq(values);
        let expected_pq = [0.39038754, 0.41710275, 0.4801437];
        for (actual, expected) in pq.into_iter().zip(expected_pq) {
            assert!((actual - expected).abs() < 0.000001);
        }

        let round_trip = pq_to_scrgb(pq);

        for (actual, expected) in round_trip.into_iter().zip(values) {
            assert!((actual - expected).abs() < 0.001);
        }
    }

    #[test]
    fn present_plan_preserves_non_default_domain_for_shader_constants() {
        let cube_path = write_test_cube_contents(
            "LUT_3D_SIZE 2\n\
DOMAIN_MIN -1.0 0.0 0.0\n\
DOMAIN_MAX 1.0 1.0 1.0\n\
0.0 0.0 0.0\n\
1.0 0.0 0.0\n\
0.0 1.0 0.0\n\
1.0 1.0 0.0\n\
0.0 0.0 1.0\n\
1.0 0.0 1.0\n\
0.0 1.0 1.0\n\
1.0 1.0 1.0\n",
        );
        let identity = MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e02,
            },
            target_id: 4357,
        };
        let mut manifest = LutManifest::empty();
        manifest.add(LutAssignment {
            target: MonitorTarget {
                identity,
                color_mode: ColorMode::Sdr,
            },
            lut_path: cube_path.clone(),
        });

        let runtime = LutPipeline::load(&manifest).expect("runtime should load");
        let plan = runtime
            .build_present_plan_for_monitor_identity(
                identity,
                ClipBox {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                DXGI_FORMAT_B8G8R8A8_UNORM,
                &[],
            )
            .expect("plan should exist");

        assert_eq!(plan.shader_constants.domain_min, [-1.0, 0.0, 0.0, 0.0]);
        assert_eq!(plan.shader_constants.domain_max, [1.0, 1.0, 1.0, 0.0]);
        assert!(runtime.shader.source.contains("domain_min"));
        assert!(runtime.shader.source.contains("NormalizeAxis"));
        assert!(runtime.shader.source.contains("NormalizeSample"));
        assert!(runtime.shader.source.contains("BlueNoiseThreshold"));
        assert!(runtime.shader.source.contains("ApplySdrDither"));
        assert!(runtime.shader.source.contains("ScrgbToPq"));
        assert!(runtime.shader.source.contains("PqToScrgb"));
        assert!(runtime.shader.source.contains("scrgb_to_bt2100"));
        assert!(runtime.shader.source.contains("bt2100_to_scrgb"));
        assert!(runtime.shader.source.contains("blue_noise_64x64"));
        assert!(runtime.shader.source.contains("max_value - min_value"));
        assert!(!runtime.shader.source.contains("safe_range"));

        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn shader_constants_cbuffer_matches_hlsl_layout() {
        let cbuffer = ShaderConstants {
            lut_size: 33,
            hdr: 1,
            domain_min: [-1.0, 0.0, 0.0, 0.0],
            domain_max: [1.0, 1.0, 1.0, 0.0],
        }
        .to_cbuffer();

        let base = (&cbuffer as *const ShaderConstantsCBuffer) as usize;
        assert_eq!(size_of::<ShaderConstantsCBuffer>(), 48);
        assert_eq!(addr_of!(cbuffer.lut_size) as usize - base, 0);
        assert_eq!(addr_of!(cbuffer.hdr) as usize - base, 4);
        assert_eq!(addr_of!(cbuffer.padding) as usize - base, 8);
        assert_eq!(addr_of!(cbuffer.domain_min) as usize - base, 16);
        assert_eq!(addr_of!(cbuffer.domain_max) as usize - base, 32);
        assert_eq!(cbuffer.padding, [0.0, 0.0]);
    }
}
