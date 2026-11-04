//! SVG pages -> multi-page PDF (vector, no rasterization).

use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref};
use std::collections::HashMap;
use svg2pdf::usvg;

/// Bravura (SMuFL music font, SIL OFL 1.1) so glyphs can be converted to outlines.
const BRAVURA: &[u8] = include_bytes!("../../public/fonts/Bravura.otf");

/// A4 in PDF points.
const PAGE_W: f32 = 595.28;
const PAGE_H: f32 = 841.89;

pub fn svgs_to_pdf(svgs: &[String]) -> Result<Vec<u8>, String> {
    if svgs.is_empty() {
        return Err("no pages to export".into());
    }
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    options.fontdb_mut().load_font_data(BRAVURA.to_vec());

    let mut alloc = Ref::new(1);
    let catalog_id = alloc.bump();
    let page_tree_id = alloc.bump();
    let mut pdf = Pdf::new();

    struct PageRefs {
        page: Ref,
        content: Ref,
        xobject: Ref,
    }
    let mut pages: Vec<PageRefs> = Vec::new();

    for svg in svgs {
        let tree = usvg::Tree::from_str(svg, &options).map_err(|e| format!("invalid SVG page: {e}"))?;
        let (chunk, root) = svg2pdf::to_chunk(&tree, svg2pdf::ConversionOptions::default()).map_err(|e| format!("PDF conversion failed: {e}"))?;
        let mut map = HashMap::new();
        let chunk = chunk.renumber(|old| *map.entry(old).or_insert_with(|| alloc.bump()));
        let xobject = *map.get(&root).ok_or("missing XObject")?;
        pdf.extend(&chunk);
        pages.push(PageRefs { page: alloc.bump(), content: alloc.bump(), xobject });
    }

    pdf.catalog(catalog_id).pages(page_tree_id);
    pdf.pages(page_tree_id).kids(pages.iter().map(|p| p.page)).count(pages.len() as i32);

    let name = Name(b"S1");
    for p in &pages {
        let mut page = pdf.page(p.page);
        page.media_box(Rect::new(0.0, 0.0, PAGE_W, PAGE_H));
        page.parent(page_tree_id);
        page.contents(p.content);
        page.resources().x_objects().pair(name, p.xobject);
        page.finish();

        let mut content = Content::new();
        content.transform([PAGE_W, 0.0, 0.0, PAGE_H, 0.0, 0.0]).x_object(name);
        pdf.stream(p.content, &content.finish());
    }
    Ok(pdf.finish())
}
