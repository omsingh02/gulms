//! Turns a lecture deck into clean, study-ready Markdown: titles, bullet
//! hierarchy, tables, flowcharts (as Mermaid) and figure cards.
//!
//! Port of the original Python extractor. It reads the same signals (deck-wide
//! repeated shapes are template chrome, university branding is filtered by text
//! and image hash) so notes come out the same way.

use regex::{Regex, RegexBuilder};
use roxmltree::{Document, Node};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use crate::pptx::{
    self, Geom, Inheritance, NS_A, NS_P, NS_R, Package, child, descendant, is_el, own_geom,
    placeholder_of, shape_tree, shapes_of,
};

/// SHA-256 prefixes of the university's template images (badges, banners, watermarks).
const TEMPLATE_CORPUS_PREFIXES: &[&str] = &[
    "305ff7cbda57d943",
    "647386bf650a0065",
    "4dc6cc05a8b1d55f",
    "0f70c06e01d10737",
    "551b16443b938701",
    "ca399f9ba7eec999",
    "249ed19ed3a885e2",
    "b4f99aeb9e9b234c",
];

const TEMPLATE_TEXT_PATTERNS: &[&str] = &[
    r"galgotias\s+university",
    r"school\s+of\s+computing\s+science",
    r"department\s+of\s+computer\s+science",
    r"\bg-?scale\b",
    r"student-?centred\s+active\s+learning\s+ecosystem",
    r"think-?pair-?share",
    r"course\s+code\s*:\s*[A-Z0-9]+",
    r"session\s*:\s*202\d",
    r"program\s*:\s*b\.?tech",
    r"^\s*\[\s*\d+\s*-?\s*mins?\s*\]\s*$",
    r"^\s*\d+\s*$",
];

static TEMPLATE_TEXT_RE: LazyLock<Regex> = LazyLock::new(|| {
    RegexBuilder::new(&TEMPLATE_TEXT_PATTERNS.join("|"))
        .case_insensitive(true)
        .build()
        .expect("static regex")
});
static RE_SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// Bin size for the "same place on most slides" test: ~2 mm in EMU.
const CHROME_BIN_EMU: i64 = 180_000;
/// A shape sitting at the same spot on at least this share of slides is template chrome.
const CHROME_FREQUENCY: f64 = 0.70;
/// Squared-distance-free cut-off for attaching a branch label to an arrow (EMU).
const LABEL_REACH_EMU: f64 = 1_200_000.0;

pub fn is_template_text(text: &str) -> bool {
    let cleaned = text.trim();
    cleaned.chars().count() < 2 || TEMPLATE_TEXT_RE.is_match(cleaned)
}

pub fn is_template_asset(img_bytes: &[u8]) -> bool {
    let hex: String = Sha256::digest(img_bytes)
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    TEMPLATE_CORPUS_PREFIXES.iter().any(|p| hex.starts_with(p))
}

fn dist_to_bbox(point: (f64, f64), bbox: [i64; 4]) -> f64 {
    let [x, y, w, h] = bbox.map(|v| v as f64);
    let dx = (x - point.0).max(0.0).max(point.0 - (x + w));
    let dy = (y - point.1).max(0.0).max(point.1 - (y + h));
    dx.hypot(dy)
}

// ------------------------------------------------------------------ data model

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GraphNode {
    pub id: String,
    pub text: String,
    pub geom: String,
    pub bbox: [i64; 4],
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Figure {
    pub size: [u32; 2],
    pub path: String,
    pub ocr_sample: String,
    pub ocr_lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SlideData {
    pub slide_num: usize,
    pub title: String,
    pub text_blocks: Vec<(usize, String)>,
    pub tables: Vec<Vec<Vec<String>>>,
    pub graph_nodes: Vec<GraphNode>,
    pub graph_edges: Vec<(String, String, String)>,
    pub figures: Vec<Figure>,
    pub markdown: String,
}

#[derive(Debug, Clone, Default)]
pub struct ExtractOptions {
    /// Where to save extracted figures (created on demand). `None` keeps them in memory only.
    pub figures_dir: Option<PathBuf>,
    /// Prefix used for figure links in the Markdown (relative to the notes file).
    pub figures_href: String,
    /// Read text out of figures with `tesseract`, when it is installed.
    pub ocr: bool,
    /// Only emit a flowchart when at least one arrow connects the boxes. Without this,
    /// any two text boxes on a slide are reported as a diagram of unconnected nodes.
    pub require_edges: bool,
    /// Keep pictures that sit in a content placeholder. The original extractor skipped
    /// them (python-pptx labels them "placeholder", not "picture"), losing real figures.
    pub include_placeholder_pictures: bool,
    /// Read PowerPoint connectors (`p:cxnSp`) as arrows, honouring the shapes they are
    /// attached to and their arrowheads, and hide the "yes"/"no" label boxes they carry.
    /// The original only recognised arrows drawn as plain shapes.
    pub improve_diagrams: bool,
    /// Turn soft line breaks inside a bullet or table cell into spaces. They are stored
    /// as invisible vertical tabs, which glued words together ("onesecond").
    pub soft_breaks_as_spaces: bool,
}

impl ExtractOptions {
    /// Settings for real use: every known quality fix on. `Default` instead reproduces
    /// the original Python output exactly, which is what the parity test checks.
    pub fn recommended() -> Self {
        Self {
            require_edges: true,
            include_placeholder_pictures: true,
            improve_diagrams: true,
            soft_breaks_as_spaces: true,
            ..Self::default()
        }
    }
}

// ------------------------------------------------------------------ text access

/// Text of one `a:p`: runs and fields concatenated, `a:br` as a vertical tab.
fn paragraph_text(p: Node) -> String {
    let mut out = String::new();
    for c in p.children().filter(|c| c.is_element()) {
        if c.tag_name().namespace() != Some(NS_A) {
            continue;
        }
        match c.tag_name().name() {
            "r" | "fld" => {
                if let Some(t) = child(c, NS_A, "t") {
                    out.push_str(t.text().unwrap_or_default());
                }
            }
            "br" => out.push('\u{b}'),
            _ => {}
        }
    }
    out
}

fn paragraph_level(p: Node) -> usize {
    child(p, NS_A, "pPr")
        .and_then(|pr| pr.attribute("lvl"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn paragraphs<'a, 'i>(body: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    body.children().filter(|c| is_el(*c, NS_A, "p"))
}

/// A text frame's paragraphs joined by newlines (`txBody` of a shape or a table cell).
fn text_frame_text(tx_body: Option<Node>) -> String {
    tx_body
        .map(|b| {
            paragraphs(b)
                .map(paragraph_text)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn soft_breaks(text: &str, opts: &ExtractOptions) -> String {
    if opts.soft_breaks_as_spaces {
        text.replace('\u{b}', " ")
    } else {
        text.to_string()
    }
}

fn collapse_spaces(s: &str) -> String {
    RE_SPACES.replace_all(s, " ").into_owned()
}

// ----------------------------------------------------------------- the diagram

/// `"".join(element.itertext())` as it behaves on pretty-printed XML. The original
/// serialised slides with indentation before reading them back, so whitespace
/// sits between sibling elements; reproducing that keeps node text identical.
fn pretty_itertext(node: Node, depth: usize, indent: bool, out: &mut String) {
    let kids: Vec<Node> = node
        .children()
        .filter(|c| {
            c.is_element()
                || c.is_text()
                    && (!c.text().unwrap_or_default().trim().is_empty()
                        || node.children().count() == 1)
        })
        .collect();
    if kids.is_empty() {
        return;
    }
    let format = indent && !kids.iter().any(|c| c.is_text());
    if format {
        out.push('\n');
    }
    for c in kids {
        if c.is_text() {
            out.push_str(c.text().unwrap_or_default());
        } else {
            if format {
                out.push_str(&"  ".repeat(depth + 1));
            }
            pretty_itertext(c, depth + 1, format, out);
            if format {
                out.push('\n');
            }
        }
    }
    if format {
        out.push_str(&"  ".repeat(depth));
    }
}

fn depth_of(node: Node) -> usize {
    node.ancestors().count() - 2 // not the document root, and not the node itself
}

fn int_attr(node: Option<Node>, name: &str) -> i64 {
    node.and_then(|n| n.attribute(name))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

type Edges = Vec<(String, String, String)>;

/// Boxes and arrows drawn with shapes and connectors, as nodes and labelled edges,
/// plus the text of the small "yes"/"no" label boxes.
fn extract_diagram_graph(doc: &Document, improve: bool) -> (Vec<GraphNode>, Edges, Vec<String>) {
    struct Line {
        p1: (f64, f64),
        p2: (f64, f64),
        /// Shape ids a connector is explicitly glued to.
        start_id: Option<String>,
        end_id: Option<String>,
        /// Arrowhead at the start only: the edge points the other way.
        reversed: bool,
    }
    struct Label {
        text: String,
        center: (f64, f64),
    }

    let mut nodes: Vec<GraphNode> = Vec::new();
    let mut lines: Vec<Line> = Vec::new();
    let mut labels: Vec<Label> = Vec::new();

    for el in doc.descendants().filter(|n| n.is_element()) {
        let is_sp = is_el(el, NS_P, "sp");
        let is_cxn = improve && is_el(el, NS_P, "cxnSp");
        if !is_sp && !is_cxn {
            continue;
        }
        let sp = el;

        let nv = descendant(sp, NS_P, if is_cxn { "nvCxnSpPr" } else { "nvSpPr" });
        let cnv = nv.and_then(|n| descendant(n, NS_P, "cNvPr"));
        let sp_id = cnv.and_then(|c| c.attribute("id")).unwrap_or_default();
        let name = cnv.and_then(|c| c.attribute("name")).unwrap_or_default();

        let mut raw = String::new();
        pretty_itertext(sp, depth_of(sp), true, &mut raw);
        let tx = raw.trim().to_string();

        let geom = descendant(sp, NS_A, "prstGeom")
            .map(|g| g.attribute("prst").unwrap_or("custom").to_string())
            .unwrap_or_else(|| "custom".to_string());

        let Some(xfrm) = descendant(sp, NS_A, "xfrm") else {
            continue;
        };
        let (Some(off), Some(ext)) = (descendant(xfrm, NS_A, "off"), descendant(xfrm, NS_A, "ext"))
        else {
            continue;
        };

        let (x, y) = (int_attr(Some(off), "x"), int_attr(Some(off), "y"));
        let (w, h) = (int_attr(Some(ext), "cx"), int_attr(Some(ext), "cy"));

        let is_line =
            is_cxn || geom == "line" || w < 1000 || h < 1000 || name.contains("Connector");
        let flip_h = xfrm.attribute("flipH") == Some("1");
        let flip_v = xfrm.attribute("flipV") == Some("1");

        if is_line && tx.is_empty() {
            let x1 = if flip_h { x + w } else { x };
            let y1 = if flip_v { y + h } else { y };
            let x2 = if flip_h { x } else { x + w };
            let y2 = if flip_v { y } else { y + h };

            let glued = |tag: &str| {
                descendant(sp, NS_A, tag)
                    .and_then(|c| c.attribute("id"))
                    .map(str::to_string)
            };
            let has_arrow = |tag: &str| {
                descendant(sp, NS_A, tag)
                    .is_some_and(|e| e.attribute("type").is_some_and(|t| t != "none"))
            };
            lines.push(Line {
                p1: (x1 as f64, y1 as f64),
                p2: (x2 as f64, y2 as f64),
                start_id: if is_cxn { glued("stCxn") } else { None },
                end_id: if is_cxn { glued("endCxn") } else { None },
                reversed: is_cxn && has_arrow("headEnd") && !has_arrow("tailEnd"),
            });
        } else if !tx.is_empty() {
            let low = tx.to_lowercase();
            let is_branch_label =
                matches!(low.as_str(), "true" | "false" | "yes" | "no" | "0" | "1")
                    || (tx.chars().count() < 10 && geom == "rect" && w < 1_000_000);
            if is_branch_label {
                labels.push(Label {
                    text: tx.clone(),
                    center: (x as f64 + w as f64 / 2.0, y as f64 + h as f64 / 2.0),
                });
            } else if !is_template_text(&tx)
                && (geom.starts_with("flowChart")
                    || matches!(geom.as_str(), "rect" | "roundRect" | "diamond" | "ellipse"))
            {
                nodes.push(GraphNode {
                    id: format!("node_{sp_id}"),
                    text: collapse_spaces(&tx).replace('"', "'").trim().to_string(),
                    geom,
                    bbox: [x, y, w, h],
                });
            }
        }
    }

    let label_texts: Vec<String> = labels.iter().map(|l| l.text.clone()).collect();
    if nodes.len() < 2 {
        return (Vec::new(), Vec::new(), label_texts);
    }

    let closest = |p: (f64, f64)| -> &GraphNode {
        nodes
            .iter()
            .min_by(|a, b| dist_to_bbox(p, a.bbox).total_cmp(&dist_to_bbox(p, b.bbox)))
            .expect("at least two nodes")
    };
    let by_shape_id = |id: &Option<String>| -> Option<&GraphNode> {
        let wanted = format!("node_{}", id.as_deref()?);
        nodes.iter().find(|n| n.id == wanted)
    };

    let mut edges: Edges = Vec::new();
    for line in &lines {
        let start = by_shape_id(&line.start_id).unwrap_or_else(|| closest(line.p1));
        let end = by_shape_id(&line.end_id).unwrap_or_else(|| closest(line.p2));
        if start.id == end.id {
            continue;
        }
        let mid = ((line.p1.0 + line.p2.0) / 2.0, (line.p1.1 + line.p2.1) / 2.0);
        let label = labels
            .iter()
            .find(|l| (l.center.0 - mid.0).hypot(l.center.1 - mid.1) < LABEL_REACH_EMU)
            .map(|l| l.text.clone())
            .unwrap_or_default();
        let (from, to) = if line.reversed {
            (end, start)
        } else {
            (start, end)
        };
        let edge = (from.id.clone(), to.id.clone(), label);
        if !edges.contains(&edge) {
            edges.push(edge);
        }
    }

    if improve && !edges.is_empty() {
        // Boxes that no arrow touches are headings or notes, not part of the flow.
        let connected: std::collections::HashSet<&str> = edges
            .iter()
            .flat_map(|(from, to, _)| [from.as_str(), to.as_str()])
            .collect();
        nodes.retain(|n| connected.contains(n.id.as_str()));
    }

    (nodes, edges, label_texts)
}

// -------------------------------------------------------------------- figures

fn extension_for(bytes: &[u8]) -> &'static str {
    use imagesize::ImageType as T;
    match imagesize::image_type(bytes) {
        Ok(T::Jpeg) => "jpg",
        Ok(T::Gif) => "gif",
        Ok(T::Bmp) => "bmp",
        Ok(T::Tiff) => "tiff",
        Ok(T::Webp) => "webp",
        _ => "png",
    }
}

pub fn tesseract_available() -> bool {
    static FOUND: LazyLock<bool> = LazyLock::new(|| {
        Command::new("tesseract")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    });
    *FOUND
}

fn ocr_image(path: &Path) -> String {
    Command::new("tesseract")
        .arg(path)
        .arg("stdout")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Keep a non-branding figure: save it, and read any text inside it.
fn handle_figure(
    blob: &[u8],
    slide_num: usize,
    fig_idx: usize,
    opts: &ExtractOptions,
) -> Option<Figure> {
    let size = imagesize::blob_size(blob).ok()?;
    let (w, h) = (size.width as u32, size.height as u32);
    if w < 100 || h < 60 {
        return None; // icon slivers
    }

    let mut saved: Option<PathBuf> = None;
    let mut href = String::new();
    if let Some(dir) = &opts.figures_dir {
        std::fs::create_dir_all(dir).ok()?;
        let name = format!(
            "fig_s{:02}_{:02}.{}",
            slide_num,
            fig_idx,
            extension_for(blob)
        );
        let target = dir.join(&name);
        std::fs::write(&target, blob).ok()?;
        href = if opts.figures_href.is_empty() {
            name
        } else {
            format!("{}/{}", opts.figures_href.trim_end_matches('/'), name)
        };
        saved = Some(target);
    }

    let mut ocr_text = String::new();
    if opts.ocr && tesseract_available() {
        match &saved {
            Some(path) => ocr_text = ocr_image(path),
            None => {
                let tmp = std::env::temp_dir().join(format!(
                    "gulms-ocr-{}-{}-{}.{}",
                    std::process::id(),
                    slide_num,
                    fig_idx,
                    extension_for(blob)
                ));
                if std::fs::write(&tmp, blob).is_ok() {
                    ocr_text = ocr_image(&tmp);
                    let _ = std::fs::remove_file(&tmp);
                }
            }
        }
    }

    Some(Figure {
        size: [w, h],
        path: href,
        ocr_sample: ocr_text
            .chars()
            .take(200)
            .collect::<String>()
            .replace('\n', " "),
        ocr_lines: ocr_text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
    })
}

// ------------------------------------------------------------------- markdown

fn render_markdown(
    left_to_right: bool,
    title: &str,
    text_blocks: &[(usize, String)],
    tables: &[Vec<Vec<String>>],
    graph_nodes: &[GraphNode],
    graph_edges: &[(String, String, String)],
    figures: &[Figure],
) -> String {
    let mut lines: Vec<String> = Vec::new();

    if !title.is_empty() {
        lines.push(format!("# {title}\n"));
    }

    for (indent, txt) in text_blocks {
        lines.push(format!("{}- {}", "  ".repeat(*indent), txt));
    }

    for tbl in tables {
        let cols = tbl[0].len();
        lines.push(format!("\n| {} |", tbl[0].join(" | ")));
        lines.push(format!("| {} |", vec!["---"; cols].join(" | ")));
        for row in &tbl[1..] {
            let mut padded = row.clone();
            padded.resize(padded.len().max(cols), String::new());
            padded.truncate(cols);
            lines.push(format!("| {} |", padded.join(" | ")));
        }
        lines.push(String::new());
    }

    if !graph_nodes.is_empty() {
        lines.push(format!(
            "\n```mermaid\nflowchart {}",
            if left_to_right { "LR" } else { "TD" }
        ));
        for n in graph_nodes {
            let wrapped = match n.geom.as_str() {
                "flowChartDecision" => format!("{{\"{}\"}}", n.text),
                "roundRect" => format!("(\"{}\")", n.text),
                _ => format!("[\"{}\"]", n.text),
            };
            lines.push(format!("    {}{}", n.id, wrapped));
        }
        for (u, v, lbl) in graph_edges {
            let arrow = if lbl.is_empty() {
                " --> ".to_string()
            } else {
                format!(" -- {lbl} --> ")
            };
            lines.push(format!("    {u}{arrow}{v}"));
        }
        lines.push("```\n".to_string());
    }

    for (idx, fig) in figures.iter().enumerate() {
        let caption = format!("Technical Figure ({}x{} px)", fig.size[0], fig.size[1]);
        lines.push(format!("\n> **[Figure {}]**: {}", idx + 1, caption));
        if !fig.path.is_empty() {
            lines.push(format!("> ![{}]({})", caption, fig.path));
        }
        if !fig.ocr_sample.is_empty() {
            lines.push(format!("> *Embedded Labels*: {}", fig.ocr_sample));
        }
    }

    lines.join("\n").trim().to_string()
}

// ----------------------------------------------------------------- extraction

/// Shapes with group containers unrolled, in document order.
fn flatten<'a, 'i>(tree: Node<'a, 'i>, out: &mut Vec<Node<'a, 'i>>) {
    for shape in shapes_of(tree) {
        if shape.tag_name().name() == "grpSp" {
            flatten(shape, out);
        } else {
            out.push(shape);
        }
    }
}

type Bin = (i64, i64, i64, i64);

fn bin_of(geom: Geom) -> Option<Bin> {
    Some((
        geom.left?.div_euclid(CHROME_BIN_EMU),
        geom.top?.div_euclid(CHROME_BIN_EMU),
        geom.width?.div_euclid(CHROME_BIN_EMU),
        geom.height?.div_euclid(CHROME_BIN_EMU),
    ))
}

/// Position of a shape as python-pptx reports it: placeholders (shapes and
/// pictures) inherit missing values from their layout, then master.
fn effective_geom(shape: Node, inheritance: &Inheritance) -> Geom {
    let own = own_geom(shape);
    match (shape.tag_name().name(), placeholder_of(shape)) {
        ("sp" | "pic", Some(ph)) => own.or(inheritance.geom_for(ph.idx)),
        _ => own,
    }
}

struct SlideInput {
    xml: String,
    inheritance: Inheritance,
    rels: HashMap<String, pptx::Rel>,
}

pub fn extract_deck(bytes: &[u8], opts: &ExtractOptions) -> Result<Vec<SlideData>, String> {
    let mut pkg = Package::open(bytes)?;
    let slide_paths = pkg.slide_paths();
    if slide_paths.is_empty() {
        return Err("the file contains no slides".to_string());
    }

    let mut layout_cache = HashMap::new();
    let inputs: Vec<Option<SlideInput>> = slide_paths
        .iter()
        .map(|path| {
            let xml = pkg.read_string(path)?;
            Some(SlideInput {
                xml,
                inheritance: pkg.inheritance_for(path, &mut layout_cache),
                rels: pkg.rels(path),
            })
        })
        .collect();

    let docs: Vec<Option<Document>> = inputs
        .iter()
        .map(|i| i.as_ref().and_then(|i| Document::parse(&i.xml).ok()))
        .collect();

    // Shapes that repeat at the same spot on most slides are template chrome.
    let mut counts: HashMap<Bin, usize> = HashMap::new();
    for (input, doc) in inputs.iter().zip(&docs) {
        let (Some(input), Some(doc)) = (input, doc) else {
            continue;
        };
        let Some(tree) = shape_tree(doc) else {
            continue;
        };
        for shape in shapes_of(tree) {
            if let Some(bin) = bin_of(effective_geom(shape, &input.inheritance)) {
                *counts.entry(bin).or_default() += 1;
            }
        }
    }
    let total = slide_paths.len().max(1) as f64;
    let frequencies: HashMap<Bin, f64> = counts
        .into_iter()
        .map(|(b, c)| (b, c as f64 / total))
        .collect();

    let mut result = Vec::with_capacity(slide_paths.len());
    for (i, (input, doc)) in inputs.iter().zip(&docs).enumerate() {
        let slide_num = i + 1;
        let data = match (input, doc) {
            (Some(input), Some(doc)) => {
                extract_slide(&mut pkg, input, doc, slide_num, &frequencies, opts)
            }
            _ => empty_slide(slide_num),
        };
        result.push(data);
    }
    Ok(result)
}

fn empty_slide(slide_num: usize) -> SlideData {
    SlideData {
        slide_num,
        title: String::new(),
        text_blocks: Vec::new(),
        tables: Vec::new(),
        graph_nodes: Vec::new(),
        graph_edges: Vec::new(),
        figures: Vec::new(),
        markdown: String::new(),
    }
}

fn extract_slide(
    pkg: &mut Package,
    input: &SlideInput,
    doc: &Document,
    slide_num: usize,
    frequencies: &HashMap<Bin, f64>,
    opts: &ExtractOptions,
) -> SlideData {
    let Some(tree) = shape_tree(doc) else {
        return empty_slide(slide_num);
    };

    // 1. Title: the first top-level placeholder with idx 0.
    let title_node = shapes_of(tree).find(|s| placeholder_of(*s).is_some_and(|ph| ph.idx == 0));
    let mut title = String::new();
    if let Some(t) = title_node.filter(|t| t.tag_name().name() == "sp") {
        let raw = text_frame_text(child(t, NS_P, "txBody"));
        let raw = raw.trim();
        if !is_template_text(raw) {
            title = collapse_spaces(raw);
        }
    }

    // 2. Everything else
    let mut all_shapes = Vec::new();
    flatten(tree, &mut all_shapes);

    let mut text_blocks: Vec<(usize, String)> = Vec::new();
    let mut tables: Vec<Vec<Vec<String>>> = Vec::new();
    let mut figures: Vec<Figure> = Vec::new();

    let (mut graph_nodes, mut graph_edges, label_texts) =
        extract_diagram_graph(doc, opts.improve_diagrams);
    if opts.require_edges && graph_edges.is_empty() {
        graph_nodes.clear();
        graph_edges.clear();
    }
    // Label boxes are shown on the arrows they belong to, not repeated as bullets.
    let hide_labels = opts.improve_diagrams && !graph_edges.is_empty();

    for shape in &all_shapes {
        if bin_of(effective_geom(*shape, &input.inheritance))
            .and_then(|b| frequencies.get(&b))
            .is_some_and(|f| *f >= CHROME_FREQUENCY)
        {
            continue; // deck chrome
        }

        match shape.tag_name().name() {
            "graphicFrame" => {
                if let Some(table) = table_of(*shape, opts) {
                    tables.push(table);
                }
            }
            "pic" => {
                if placeholder_of(*shape).is_some() && !opts.include_placeholder_pictures {
                    continue;
                }
                let blob = descendant(*shape, NS_P, "blipFill")
                    .and_then(|bf| child(bf, NS_A, "blip"))
                    .and_then(|blip| blip.attribute((NS_R, "embed")))
                    .and_then(|id| input.rels.get(id))
                    .and_then(|rel| pkg.read_bytes(&rel.target));
                if let Some(blob) = blob
                    && !is_template_asset(&blob)
                    && let Some(fig) = handle_figure(&blob, slide_num, figures.len() + 1, opts)
                {
                    figures.push(fig);
                }
            }
            "sp" => {
                if title_node.is_some_and(|t| t.id() == shape.id()) {
                    continue;
                }
                let body = child(*shape, NS_P, "txBody");
                let whole = text_frame_text(body);
                if graph_nodes.iter().any(|n| n.text == whole.trim()) {
                    continue; // already shown as a diagram node
                }
                if hide_labels && label_texts.iter().any(|l| l == whole.trim()) {
                    continue; // already shown on an arrow
                }
                if let Some(body) = body {
                    for p in paragraphs(body) {
                        let text = paragraph_text(p).trim().to_string();
                        if !text.is_empty() && !is_template_text(&text) {
                            text_blocks.push((paragraph_level(p), soft_breaks(&text, opts)));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Fallback title: the first short line, when the slide has no title placeholder.
    if title.is_empty()
        && let Some((_, first)) = text_blocks.first()
        && first.chars().count() < 80
        && !is_template_text(first)
    {
        title = first.clone();
        text_blocks.remove(0);
    }

    // Screenshot-only slides (e.g. exported from Canva): fall back to OCR text.
    if text_blocks.is_empty() && graph_nodes.is_empty() && tables.is_empty() && !figures.is_empty()
    {
        for fig in &figures {
            if fig.ocr_lines.len() >= 3 {
                let content: &[String] = if title.is_empty() {
                    title = fig.ocr_lines[0].clone();
                    &fig.ocr_lines[1..]
                } else {
                    &fig.ocr_lines
                };
                for line in content {
                    if !is_template_text(line) {
                        text_blocks.push((0, line.clone()));
                    }
                }
            }
        }
    }

    let left_to_right = opts.improve_diagrams && is_wider_than_tall(&graph_nodes);
    let markdown = render_markdown(
        left_to_right,
        &title,
        &text_blocks,
        &tables,
        &graph_nodes,
        &graph_edges,
        &figures,
    );
    SlideData {
        slide_num,
        title,
        text_blocks,
        tables,
        graph_nodes,
        graph_edges,
        figures,
        markdown,
    }
}

/// Do the diagram's boxes spread out more across the slide than down it? Then a
/// left-to-right layout matches how the slide was drawn and stays compact.
fn is_wider_than_tall(nodes: &[GraphNode]) -> bool {
    if nodes.len() < 2 {
        return false;
    }
    let centers = nodes.iter().map(|n| {
        (
            n.bbox[0] as f64 + n.bbox[2] as f64 / 2.0,
            n.bbox[1] as f64 + n.bbox[3] as f64 / 2.0,
        )
    });
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for (x, y) in centers {
        (min_x, max_x, min_y, max_y) = (min_x.min(x), max_x.max(x), min_y.min(y), max_y.max(y));
    }
    (max_x - min_x) > (max_y - min_y) * 1.2
}

/// Rows of a native table (`a:tbl`) as trimmed single-line cells; empty rows dropped.
fn table_of(frame: Node, opts: &ExtractOptions) -> Option<Vec<Vec<String>>> {
    let tbl = descendant(frame, NS_A, "tbl")?;
    let rows: Vec<Vec<String>> = tbl
        .children()
        .filter(|r| is_el(*r, NS_A, "tr"))
        .map(|tr| {
            tr.children()
                .filter(|c| is_el(*c, NS_A, "tc"))
                .map(|tc| {
                    soft_breaks(
                        &text_frame_text(child(tc, NS_A, "txBody"))
                            .trim()
                            .replace('\n', " "),
                        opts,
                    )
                })
                .collect::<Vec<_>>()
        })
        .filter(|cells| cells.iter().any(|c| !c.is_empty()))
        .collect();
    (!rows.is_empty()).then_some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_text_is_recognised_and_content_is_not() {
        for chrome in [
            "Galgotias University",
            "G-SCALE",
            "Student-Centred Active Learning Ecosystem",
            "Think-Pair-Share",
            "[5-mins]",
            "23",
        ] {
            assert!(is_template_text(chrome), "{chrome} should be chrome");
        }
        for content in [
            "Database System Vs File System",
            "Flowchart for the for loop",
            "Addr(A[Index]) = B + W * (Index - LB)",
        ] {
            assert!(!is_template_text(content), "{content} should be kept");
        }
    }

    #[test]
    fn branding_images_are_recognised_by_hash_only() {
        assert!(!is_template_asset(b"test_bytes"));
        assert_eq!(TEMPLATE_CORPUS_PREFIXES.len(), 8);
    }

    #[test]
    fn distance_to_a_box_is_zero_inside_and_euclidean_outside() {
        let bbox = [0, 0, 10, 10];
        assert_eq!(dist_to_bbox((5.0, 5.0), bbox), 0.0);
        assert_eq!(dist_to_bbox((13.0, 14.0), bbox), 5.0);
    }

    /// Compares every slide against the original Python extractor on real decks.
    /// GULMS_PARITY_DECKS=/decks GULMS_PARITY_EXTRACT=/ref_extract \
    ///   cargo test --release parity_with_python_extractor -- --ignored --nocapture
    #[test]
    #[ignore]
    fn parity_with_python_extractor() {
        let (Ok(decks), Ok(refs)) = (
            std::env::var("GULMS_PARITY_DECKS"),
            std::env::var("GULMS_PARITY_EXTRACT"),
        ) else {
            panic!("set GULMS_PARITY_DECKS and GULMS_PARITY_EXTRACT");
        };
        // Faithful settings, but saving figures like real use does (the original dropped
        // images it could not convert to PNG, e.g. WMF drawings, only when saving).
        let fig_dir = std::env::temp_dir().join(format!("gulms-parity-{}", std::process::id()));
        let opts = ExtractOptions {
            figures_dir: Some(fig_dir.clone()),
            figures_href: "figures".into(),
            ..Default::default()
        };

        let (mut decks_ok, mut decks_py_failed, mut slides, mut bad_slides, mut lenient) =
            (0, 0, 0, 0, 0);
        let mut by_field: std::collections::BTreeMap<&str, usize> = Default::default();
        let mut samples: Vec<String> = Vec::new();
        let figure_links = regex::Regex::new(r"> !\[[^\]]*\]\([^)]*\)\n?").unwrap();

        let mut entries: Vec<_> = std::fs::read_dir(&refs).unwrap().flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let key = entry
                .file_name()
                .to_string_lossy()
                .trim_end_matches(".json")
                .to_string();
            let reference: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(entry.path()).unwrap()).unwrap();
            if reference["ok"] != true {
                decks_py_failed += 1;
                // Python crashed on this deck; Rust must still produce output.
                let bytes = std::fs::read(format!("{decks}/{key}.pptx")).unwrap();
                assert!(
                    extract_deck(&bytes, &opts).is_ok(),
                    "{key}: rust should handle decks python crashed on"
                );
                continue;
            }
            decks_ok += 1;
            let bytes = std::fs::read(format!("{decks}/{key}.pptx")).unwrap();
            let got = extract_deck(&bytes, &opts).unwrap_or_else(|e| panic!("{key}: {e}"));
            let want = reference["slides"].as_array().unwrap();
            assert_eq!(got.len(), want.len(), "{key}: slide count");
            for (g, w) in got.iter().zip(want) {
                slides += 1;
                let g = serde_json::to_value(g).unwrap();
                let sizes = |v: &serde_json::Value| -> Vec<String> {
                    let mut s: Vec<String> = v["figures"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|f| f["size"].to_string())
                        .collect();
                    s.sort();
                    s
                };
                // Rust may keep a figure Pillow refused to identify (e.g. a PNG with a bad
                // metadata checksum); that is a leniency, not a regression.
                let (gs, ws) = (sizes(&g), sizes(w));
                let mut pool = gs.clone();
                let python_subset = ws.iter().all(|s| match pool.iter().position(|p| p == s) {
                    Some(i) => {
                        pool.remove(i);
                        true
                    }
                    None => false,
                });
                let figures_equal = gs == ws;
                let mut bad = false;
                let mut fields = vec![
                    "title",
                    "text_blocks",
                    "tables",
                    "graph_nodes",
                    "graph_edges",
                ];
                if figures_equal {
                    fields.extend(["figures", "markdown"]);
                } else if python_subset {
                    lenient += 1;
                } else {
                    fields.extend(["figures", "markdown"]);
                }
                for field in fields {
                    let (mut gv, mut wv) = (g[field].clone(), w[field].clone());
                    if field == "figures" {
                        // Paths differ by image format/extension; the content is what matters.
                        for v in [&mut gv, &mut wv] {
                            for f in v.as_array_mut().unwrap() {
                                f["path"] = serde_json::json!("");
                            }
                        }
                    }
                    if field == "markdown" {
                        let strip = |s: &serde_json::Value| {
                            figure_links
                                .replace_all(s.as_str().unwrap(), "")
                                .into_owned()
                        };
                        gv = strip(&gv).into();
                        wv = strip(&wv).into();
                    }
                    if gv != wv {
                        bad = true;
                        *by_field.entry(field).or_default() += 1;
                        if samples.len() < 6 {
                            samples.push(format!(
                                "{key} slide {} field {field}\n  want {}\n  got  {}",
                                w["slide_num"],
                                wv.to_string().chars().take(300).collect::<String>(),
                                gv.to_string().chars().take(300).collect::<String>()
                            ));
                        }
                    }
                }
                if bad {
                    bad_slides += 1;
                }
            }
        }
        let _ = std::fs::remove_dir_all(&fig_dir);
        println!(
            "decks compared {decks_ok} (python crashed on {decks_py_failed}), slides {slides}, mismatching slides {bad_slides}, by field {by_field:?}; slides where rust kept extra figures python could not read: {lenient}"
        );
        for s in &samples {
            println!("{s}");
        }
        assert_eq!(bad_slides, 0);
    }

    const SIMPLE: &[u8] = include_bytes!("../tests/fixtures/simple.pptx");
    const SIMPLE_EXPECTED: &str = include_str!("../tests/fixtures/simple.expected.json");
    const RICH: &[u8] = include_bytes!("../tests/fixtures/rich.pptx");
    const RICH_EXPECTED: &str = include_str!("../tests/fixtures/rich.expected.json");

    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gulms-extract-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Faithful settings plus a real figures folder, like the original run with an output dir.
    fn faithful(dir: &Path) -> ExtractOptions {
        ExtractOptions {
            figures_dir: Some(dir.join("figures")),
            figures_href: "figures".into(),
            ..Default::default()
        }
    }

    fn assert_matches_original(deck: &[u8], expected_json: &str, name: &str) {
        let dir = scratch_dir(name);
        let got = extract_deck(deck, &faithful(&dir)).unwrap();
        let want: Vec<serde_json::Value> = serde_json::from_str(expected_json).unwrap();
        assert_eq!(got.len(), want.len());
        for (g, w) in got.iter().zip(&want) {
            let g = serde_json::to_value(g).unwrap();
            for field in [
                "slide_num",
                "title",
                "text_blocks",
                "tables",
                "graph_nodes",
                "graph_edges",
                "figures",
                "markdown",
            ] {
                assert_eq!(g[field], w[field], "slide {} field {field}", w["slide_num"]);
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn simple_deck_matches_the_original_extractor() {
        assert_matches_original(SIMPLE, SIMPLE_EXPECTED, "simple");
    }

    #[test]
    fn rich_deck_matches_the_original_extractor() {
        assert_matches_original(RICH, RICH_EXPECTED, "rich");
    }

    #[test]
    fn simple_deck_content() {
        let slides = extract_deck(SIMPLE, &ExtractOptions::recommended()).unwrap();
        assert_eq!(slides.len(), 2);
        assert_eq!(slides[0].title, "Introduction to Relational Databases");
        assert!(slides[0].markdown.contains("ACID properties"));
        assert_eq!(slides[1].title, "Database Comparison");
        assert_eq!(slides[1].tables.len(), 1);
        for row in [
            "| Paradigm | ACID Support |",
            "| RDBMS | Strict |",
            "| NoSQL | Eventual |",
        ] {
            assert!(slides[1].markdown.contains(row), "{row}");
        }
    }

    #[test]
    fn recommended_mode_fixes_soft_breaks_diagrams_and_titles() {
        let dir = scratch_dir("recommended");
        let opts = ExtractOptions {
            figures_dir: Some(dir.join("figures")),
            figures_href: "figures".into(),
            ..ExtractOptions::recommended()
        };
        let slides = extract_deck(RICH, &opts).unwrap();

        // Soft line break no longer glues words together.
        assert!(
            slides[0]
                .markdown
                .contains("- Second line one second line two"),
            "{}",
            slides[0].markdown
        );
        assert!(!slides[0].markdown.contains('\u{b}'));
        // Hierarchy and chrome filtering are unchanged.
        assert!(slides[0].markdown.contains("    - No repeating groups"));
        assert!(!slides[0].markdown.contains("Galgotias"));

        // Table: empty row dropped, newline in a cell flattened; only the real picture is kept.
        assert_eq!(slides[1].tables[0].len(), 3);
        assert_eq!(slides[1].figures.len(), 1);
        assert_eq!(slides[1].figures[0].size, [240, 140]);
        assert!(dir.join("figures/fig_s02_01.png").is_file());

        // Flowchart: connectors become labelled arrows, the "yes" box is no longer the title.
        let flow = &slides[2];
        assert!(
            !flow.graph_edges.is_empty(),
            "connectors should produce edges"
        );
        assert!(
            flow.graph_edges.iter().any(|(_, _, label)| label == "yes"),
            "{:?}",
            flow.graph_edges
        );
        // Only the three boxes an arrow touches are in the diagram; the heading and the note
        // beside it stay as ordinary text, and the heading becomes the slide title.
        assert_eq!(flow.graph_nodes.len(), 3, "{:?}", flow.graph_nodes);
        assert_eq!(flow.title, "Flowchart for the loop");
        assert!(
            flow.text_blocks
                .iter()
                .any(|(_, t)| t == "Loop runs ten times")
        );
        assert!(
            flow.markdown.contains("flowchart LR"),
            "wide layout reads left to right: {}",
            flow.markdown
        );

        // Two boxes with no arrows are not a flowchart; their text stays as ordinary content.
        let boxes = &slides[3];
        assert!(boxes.graph_nodes.is_empty());
        assert!(!boxes.markdown.contains("mermaid"));
        assert!(boxes.markdown.contains("Left box") && boxes.markdown.contains("Right box"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn garbage_input_is_an_error_not_a_panic() {
        assert!(extract_deck(b"not a zip", &ExtractOptions::default()).is_err());
        let empty_zip = crate::analyzer::testdeck::zip_of(&[("readme.txt", b"hi".to_vec())]);
        assert!(extract_deck(&empty_zip, &ExtractOptions::default()).is_err());
    }

    /// Prints how the recommended settings change real output compared to the faithful port.
    #[test]
    #[ignore]
    fn survey_recommended_mode() {
        let decks = std::env::var("GULMS_PARITY_DECKS").expect("GULMS_PARITY_DECKS");
        let (mut slides, mut faithful_graphs, mut faithful_edges, mut rec_graphs, mut rec_edges) =
            (0, 0, 0, 0, 0);
        let (mut faithful_figs, mut rec_figs, mut soft_breaks_left) = (0, 0, 0);
        for entry in std::fs::read_dir(decks).unwrap().flatten() {
            let bytes = std::fs::read(entry.path()).unwrap();
            let (Ok(a), Ok(b)) = (
                extract_deck(&bytes, &ExtractOptions::default()),
                extract_deck(&bytes, &ExtractOptions::recommended()),
            ) else {
                continue;
            };
            for (fa, re) in a.iter().zip(&b) {
                slides += 1;
                faithful_graphs += usize::from(!fa.graph_nodes.is_empty());
                faithful_edges += usize::from(!fa.graph_edges.is_empty());
                rec_graphs += usize::from(!re.graph_nodes.is_empty());
                rec_edges += usize::from(!re.graph_edges.is_empty());
                faithful_figs += fa.figures.len();
                rec_figs += re.figures.len();
                soft_breaks_left += usize::from(re.markdown.contains('\u{b}'));
            }
        }
        println!(
            "slides {slides}\n flowcharts: faithful {faithful_graphs} (with arrows {faithful_edges}) -> recommended {rec_graphs} (with arrows {rec_edges})\n figures: faithful {faithful_figs} -> recommended {rec_figs}\n slides still containing a soft break: {soft_breaks_left}"
        );
    }
}
