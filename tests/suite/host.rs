//! Host APIs need no installed fonts, external PDF tools or private fixtures.
use std::io::{Cursor, Write};
use std::sync::{Arc, Barrier};

use docxide_pdf::{
    Compression, ConvertOptions, clear_fonts, convert_docx_bytes_to_pdf_bytes_with, register_font,
    take_font_requests, use_system_fonts,
};

const FONT: &[u8] = include_bytes!("../../src/fonts/testdata/nb-hyphen-native.ttf");

/// Only registered fonts on this thread, from a clean registry.
struct Host;

impl Host {
    fn new() -> Self {
        clear_fonts();
        use_system_fonts(false);
        take_font_requests();
        Self
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        clear_fonts();
        use_system_fonts(true);
    }
}

fn font_name() -> String {
    ttf_parser::Face::parse(FONT, 0)
        .unwrap()
        .names()
        .into_iter()
        .find(|n| n.name_id == ttf_parser::name_id::FAMILY && n.is_unicode())
        .unwrap()
        .to_string()
        .unwrap()
}

/// One paragraph in `font`, with `rpr` added to its run properties.
fn document(font: &str, rpr: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(
        "word/document.xml",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    write!(zip, r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:rPr><w:rFonts w:ascii="{font}" w:hAnsi="{font}"/>{rpr}</w:rPr><w:t>-</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="4000" w:h="4000"/><w:pgMar w:top="400" w:bottom="400" w:left="400" w:right="400"/></w:sectPr></w:body></w:document>"#).unwrap();
    zip.finish().unwrap().into_inner()
}

fn convert(input: &[u8]) -> Vec<u8> {
    let pdf = convert_docx_bytes_to_pdf_bytes_with(input, &ConvertOptions::new().font_cache(false))
        .unwrap();
    assert!(pdf.starts_with(b"%PDF-"));
    pdf
}

fn pdf_contains(pdf: &[u8], needle: &[u8]) -> bool {
    let contains = |bytes: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    contains(pdf)
        || pdf
            .windows(7)
            .enumerate()
            .filter(|(_, w)| *w == b"stream\n")
            .any(|(start, _)| {
                let data = &pdf[start + 7..];
                let Some(end) = data.windows(10).position(|w| w == b"\nendstream") else {
                    return false;
                };
                miniz_oxide::inflate::decompress_to_vec_zlib(&data[..end])
                    .is_ok_and(|data| contains(&data))
            })
}

fn requested(name: &str) -> bool {
    take_font_requests()
        .iter()
        .any(|r| r.names.iter().any(|n| n == name))
}

/// A real font subset, not only the standard-14 fallback, reached the PDF.
fn embeds_font(pdf: &[u8]) -> bool {
    pdf_contains(pdf, b"/FontFile2")
}

#[test]
fn host_fonts_can_be_registered_and_cleared() {
    let _host = Host::new();
    let doc = document(&font_name(), "");
    assert!(!embeds_font(&convert(&doc)));
    assert_eq!(register_font(vec![0, 1, 2]), 0);
    assert_eq!(register_font(FONT.to_vec()), 1);
    assert!(embeds_font(&convert(&doc)));
    clear_fonts();
    assert!(!embeds_font(&convert(&doc)));
}

#[test]
fn host_fonts_cover_a_missing_style_with_the_registered_face() {
    let _host = Host::new();
    register_font(FONT.to_vec());
    assert!(embeds_font(&convert(&document(&font_name(), "<w:b/>"))));
}

#[test]
fn host_fonts_belong_to_the_registering_thread() {
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|i| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let _host = Host::new();
                if i == 0 {
                    register_font(FONT.to_vec());
                }
                barrier.wait();
                let pdf = convert(&document(&font_name(), ""));
                assert_eq!(embeds_font(&pdf), i == 0);
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn host_requests_name_missing_fonts_until_registered() {
    let _host = Host::new();
    let name = font_name();
    let doc = document(&name, "");
    convert(&doc);
    assert!(requested(&name));
    register_font(FONT.to_vec());
    convert(&doc);
    assert!(!requested(&name));
    clear_fonts();
    convert(&doc);
    assert!(requested(&name));
    assert!(take_font_requests().is_empty());
}

#[test]
fn host_requests_exact_style_even_when_regular_is_registered() {
    let _host = Host::new();
    let name = font_name();
    register_font(FONT.to_vec());
    convert(&document(&name, "<w:b/>"));
    assert!(
        take_font_requests()
            .iter()
            .any(|r| r.bold && r.names.contains(&name))
    );
}

#[test]
fn host_requests_cleared_by_failed_conversion() {
    let _host = Host::new();
    convert(&document(&font_name(), ""));
    assert!(convert_docx_bytes_to_pdf_bytes_with(b"not a zip", &ConvertOptions::new()).is_err());
    assert!(take_font_requests().is_empty());
}

#[test]
fn host_requests_belong_to_the_converting_thread() {
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|i| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let _host = Host::new();
                if i == 0 {
                    register_font(FONT.to_vec());
                }
                barrier.wait();
                convert(&document(&font_name(), ""));
                barrier.wait();
                assert_eq!(requested(&font_name()), i == 1);
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}

/// `paragraphs` paragraphs of text in `font`.
fn long_document(font: &str, paragraphs: usize) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(
        "word/document.xml",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    write!(zip, r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>"#).unwrap();
    for i in 0..paragraphs {
        write!(zip, r#"<w:p><w:r><w:rPr><w:rFonts w:ascii="{font}" w:hAnsi="{font}"/></w:rPr><w:t>- -- --- {i} - -- ---</w:t></w:r></w:p>"#).unwrap();
    }
    write!(zip, r#"<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:bottom="1440" w:left="1440" w:right="1440"/></w:sectPr></w:body></w:document>"#).unwrap();
    zip.finish().unwrap().into_inner()
}

/// Every stream of `pdf` inflated (or as stored when it is not zlib), except
/// the cross-reference stream, whose byte offsets follow the file's size.
fn inflated_streams(pdf: &[u8]) -> Vec<Vec<u8>> {
    let mut streams: Vec<Vec<u8>> = pdf
        .windows(7)
        .enumerate()
        // `stream` opens a stream's data; `endstream` closes it.
        .filter(|&(start, w)| w == b"stream\n" && !pdf[..start].ends_with(b"end"))
        .filter(|&(start, _)| {
            let dictionary = &pdf[start.saturating_sub(300)..start];
            !dictionary.windows(5).any(|w| w == b"/XRef")
        })
        .filter_map(|(start, _)| {
            let data = &pdf[start + 7..];
            let end = data.windows(10).position(|w| w == b"\nendstream")?;
            let raw = &data[..end];
            Some(miniz_oxide::inflate::decompress_to_vec_zlib(raw).unwrap_or_else(|_| raw.to_vec()))
        })
        .collect();
    streams.sort();
    streams
}

#[test]
fn fast_compression_writes_the_same_pdf_once_inflated() {
    let _host = Host::new();
    assert!(register_font(FONT.to_vec()) > 0);
    let input = long_document(&font_name(), 400);
    let with = |compression| {
        convert_docx_bytes_to_pdf_bytes_with(
            &input,
            &ConvertOptions::new()
                .font_cache(false)
                .compression(compression),
        )
        .unwrap()
    };
    let smallest = with(Compression::Smallest);
    let fast = with(Compression::Fast);
    assert!(fast.starts_with(b"%PDF-"));
    assert_ne!(fast, smallest);
    assert!(fast.len() > smallest.len());
    let streams = inflated_streams(&smallest);
    assert!(streams.len() > 3);
    assert_eq!(inflated_streams(&fast), streams);
}
