//! Picture resolution, draft pictures and the picture cache need no installed
//! fonts, external PDF tools or private fixtures.
use std::io::{Cursor, Write};

use docxide_pdf::display::{Image, ImageFilter};
use docxide_pdf::{Compression, ConvertOptions};

/// A letter page with one 100×50pt inline picture (`word/media/image1.png`).
fn document(png: &[u8]) -> Vec<u8> {
    let body = r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="1270000" cy="635000"/><wp:docPr id="1" name="p"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="1" name="p"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rId1"/></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/></a:xfrm></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:bottom="1440" w:left="1440" w:right="1440"/></w:sectPr>"#;
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("word/document.xml", options).unwrap();
    write!(
        zip,
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body>{body}</w:body></w:document>"#
    )
    .unwrap();
    zip.start_file("word/_rels/document.xml.rels", options)
        .unwrap();
    zip.write_all(br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/></Relationships>"#).unwrap();
    zip.start_file("word/media/image1.png", options).unwrap();
    zip.write_all(png).unwrap();
    zip.finish().unwrap().into_inner()
}

/// A `width`×`height` PNG: noisy (a photograph, where JPEG wins) or two flat
/// colours (a chart, where Flate wins).
fn png(width: u32, height: u32, noisy: bool) -> Vec<u8> {
    let picture = image::RgbImage::from_fn(width, height, |x, y| {
        if noisy {
            let n =
                (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)).wrapping_mul(83_492_791);
            image::Rgb([(n >> 8) as u8, (n >> 16) as u8, (n >> 24) as u8])
        } else if x < width / 2 {
            image::Rgb([200, 30, 30])
        } else {
            image::Rgb([30, 30, 200])
        }
    });
    let mut out = Vec::new();
    picture
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

/// The PDF and the pictures it draws.
fn convert(input: &[u8], options: ConvertOptions) -> (Vec<u8>, Vec<Image>) {
    let out = docxide_pdf::convert(input, &options.font_cache(false).display_list(true)).unwrap();
    assert!(out.pdf.starts_with(b"%PDF-"));
    (out.pdf, out.display.expect("display list").images)
}

fn size(images: &[Image]) -> (u32, u32) {
    assert_eq!(images.len(), 1, "one picture");
    (images[0].width, images[0].height)
}

#[test]
fn pictures_resample_to_the_chosen_resolution() {
    // 1600×800 shown at 100×50pt: 1152 ppi.
    let input = document(&png(1600, 800, true));
    assert_eq!(size(&convert(&input, ConvertOptions::new()).1), (267, 133));
    let lower = ConvertOptions::new().picture_resolution(144.0);
    assert_eq!(size(&convert(&input, lower).1), (200, 100));
    let kept = ConvertOptions::new()
        .picture_resolution(144.0)
        .resample_pictures(false);
    assert_eq!(size(&convert(&input, kept).1), (1600, 800));
}

#[test]
fn draft_pictures_are_jpeg_without_a_lossless_trial() {
    let input = document(&png(1600, 800, false));
    let (_, images) = convert(&input, ConvertOptions::new());
    assert_eq!(
        images[0].filter,
        ImageFilter::Flate,
        "flat colours stay lossless"
    );
    let (_, images) = convert(&input, ConvertOptions::new().draft_pictures(true));
    assert_eq!(images[0].filter, ImageFilter::Jpeg);
    assert_eq!(size(&images), (267, 133));
}

#[test]
fn draft_pictures_resample_what_fast_compression_keeps() {
    // 400×200 shown at 100×50pt: 288 ppi, under fast compression's threshold
    // (twice the target), above the target itself.
    let input = document(&png(400, 200, true));
    let fast = ConvertOptions::new().compression(Compression::Fast);
    let (_, images) = convert(&input, fast.clone());
    assert_eq!(
        (size(&images), images[0].filter),
        ((400, 200), ImageFilter::Flate)
    );
    let (_, images) = convert(&input, fast.draft_pictures(true));
    assert_eq!(
        (size(&images), images[0].filter),
        ((267, 133), ImageFilter::Jpeg)
    );
}

#[test]
fn the_picture_cache_changes_nothing_but_the_work() {
    for noisy in [true, false] {
        let input = document(&png(1600, 800, noisy));
        for options in [
            ConvertOptions::new(),
            ConvertOptions::new()
                .draft_pictures(true)
                .picture_resolution(144.0),
        ] {
            let (plain, _) = convert(&input, options.clone());
            let (first, _) = convert(&input, options.clone().picture_cache(true));
            let (cached, _) = convert(&input, options.clone().picture_cache(true));
            assert_eq!(first, plain, "noisy={noisy}");
            assert_eq!(cached, plain, "noisy={noisy}");
        }
    }
}
