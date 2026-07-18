use std::error::Error;
use std::fs;
use std::io::{self, Cursor};
use std::path::Path;

const PNG_SIZE: u32 = 256;
const ICO_SIZES: &[u32] = &[16, 24, 32, 48, 256];

pub(super) fn generate() -> Result<(), Box<dyn Error>> {
    let workspace_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask is located directly under the workspace root");
    let assets_dir = workspace_dir.join("crates/dwm-lut/assets");
    let svg_path = assets_dir.join("icon.svg");
    let png_path = assets_dir.join("icon.png");
    let ico_path = assets_dir.join("icon.ico");

    let svg_data = fs::read(&svg_path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to read {}: {error}", svg_path.display()),
        )
    })?;
    let tree = resvg::usvg::Tree::from_data(&svg_data, &resvg::usvg::Options::default()).map_err(
        |error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to parse {}: {error}", svg_path.display()),
            )
        },
    )?;

    let png = render_pixmap(&tree, PNG_SIZE)?.encode_png()?;
    let ico = encode_ico(&tree, ICO_SIZES)?;
    write_file(&png_path, &png)?;
    write_file(&ico_path, &ico)?;

    println!("generated {}", png_path.display());
    println!("generated {}", ico_path.display());
    Ok(())
}

fn render_pixmap(
    tree: &resvg::usvg::Tree,
    size: u32,
) -> Result<resvg::tiny_skia::Pixmap, Box<dyn Error>> {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)
        .ok_or_else(|| io::Error::other(format!("failed to allocate {size}x{size} pixmap")))?;
    let scale = size as f32 / tree.size().width();
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(tree, transform, &mut pixmap.as_mut());
    Ok(pixmap)
}

fn encode_ico(tree: &resvg::usvg::Tree, sizes: &[u32]) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for &size in sizes {
        let png = render_pixmap(tree, size)?.encode_png()?;
        let image = ico::IconImage::read_png(Cursor::new(png))?;
        icon_dir.add_entry(ico::IconDirEntry::encode(&image)?);
    }

    let mut output = Cursor::new(Vec::new());
    icon_dir.write(&mut output)?;
    Ok(output.into_inner())
}

fn write_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    fs::write(path, contents).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to write {}: {error}", path.display()),
        )
    })
}
