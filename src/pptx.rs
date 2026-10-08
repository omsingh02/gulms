//! Just enough of the Office Open XML presentation format to read slide content:
//! the zip package, relationships, slide order and placeholder geometry.
//!
//! Behaviour follows python-pptx where the original extractor depended on it
//! (slide order, placeholder inheritance), so ported output stays comparable.

use roxmltree::{Document, Node};
use std::collections::HashMap;
use std::io::{Cursor, Read};
use zip::ZipArchive;

pub const NS_P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
pub const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
pub const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const NS_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

// ----------------------------------------------------------------- xml helpers

pub fn is_el(node: Node, ns: &str, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name && node.tag_name().namespace() == Some(ns)
}

/// First direct child element with the given namespace and name.
pub fn child<'a, 'i>(node: Node<'a, 'i>, ns: &str, name: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|c| is_el(*c, ns, name))
}

/// First descendant element (any depth), like `.//ns:name`.
pub fn descendant<'a, 'i>(node: Node<'a, 'i>, ns: &str, name: &str) -> Option<Node<'a, 'i>> {
    node.descendants().skip(1).find(|c| is_el(*c, ns, name))
}

// ------------------------------------------------------------------ geometry

/// Position and size in EMU. Each part may be unknown independently.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Geom {
    pub left: Option<i64>,
    pub top: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
}

impl Geom {
    /// Own values win; anything missing comes from `base` (placeholder inheritance).
    pub fn or(self, base: Geom) -> Geom {
        Geom {
            left: self.left.or(base.left),
            top: self.top.or(base.top),
            width: self.width.or(base.width),
            height: self.height.or(base.height),
        }
    }
}

fn int_attr(node: Node, name: &str) -> Option<i64> {
    node.attribute(name)
        .and_then(|v| v.trim().parse::<i64>().ok())
}

/// The shape's own `xfrm` (never inherited).
pub fn own_geom(shape: Node) -> Geom {
    let xfrm = match shape.tag_name().name() {
        "graphicFrame" => child(shape, NS_P, "xfrm"),
        "grpSp" => child(shape, NS_P, "grpSpPr").and_then(|g| child(g, NS_A, "xfrm")),
        _ => child(shape, NS_P, "spPr").and_then(|g| child(g, NS_A, "xfrm")),
    };
    let Some(xfrm) = xfrm else {
        return Geom::default();
    };
    let off = child(xfrm, NS_A, "off");
    let ext = child(xfrm, NS_A, "ext");
    Geom {
        left: off.and_then(|o| int_attr(o, "x")),
        top: off.and_then(|o| int_attr(o, "y")),
        width: ext.and_then(|e| int_attr(e, "cx")),
        height: ext.and_then(|e| int_attr(e, "cy")),
    }
}

// --------------------------------------------------------------- placeholders

/// `<p:ph>` details of a shape.
#[derive(Clone, Debug, PartialEq)]
pub struct Ph {
    pub idx: u32,
    pub ty: String,
}

/// The placeholder marker of a shape element, if it is a placeholder.
pub fn placeholder_of(shape: Node) -> Option<Ph> {
    let first = shape.children().find(|c| c.is_element())?;
    let ph = child(first, NS_P, "nvPr").and_then(|nv| child(nv, NS_P, "ph"))?;
    Some(Ph {
        idx: ph
            .attribute("idx")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        ty: ph.attribute("type").unwrap_or("obj").to_string(),
    })
}

/// Shapes directly under a shape tree (`p:spTree` / `p:grpSp`), in document order.
pub fn shapes_of<'a, 'i>(tree: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    tree.children().filter(|c| {
        c.is_element()
            && c.tag_name().namespace() == Some(NS_P)
            && matches!(
                c.tag_name().name(),
                "sp" | "grpSp" | "graphicFrame" | "cxnSp" | "pic" | "contentPart"
            )
    })
}

/// The `p:spTree` of a slide, layout or master document.
pub fn shape_tree<'a, 'i>(doc: &'a Document<'i>) -> Option<Node<'a, 'i>> {
    child(doc.root_element(), NS_P, "cSld").and_then(|c| child(c, NS_P, "spTree"))
}

#[derive(Clone, Debug)]
pub struct PhInfo {
    pub idx: u32,
    pub ty: String,
    pub geom: Geom,
}

/// Placeholder shapes of one layout or master.
#[derive(Clone, Debug, Default)]
pub struct Placeholders {
    items: Vec<PhInfo>,
}

impl Placeholders {
    pub fn parse(xml: &str) -> Self {
        let Ok(doc) = Document::parse(xml) else {
            return Self::default();
        };
        let Some(tree) = shape_tree(&doc) else {
            return Self::default();
        };
        let items = shapes_of(tree)
            .filter_map(|shape| {
                let ph = placeholder_of(shape)?;
                Some(PhInfo {
                    idx: ph.idx,
                    ty: ph.ty,
                    geom: own_geom(shape),
                })
            })
            .collect();
        Self { items }
    }

    fn by_idx(&self, idx: u32) -> Option<&PhInfo> {
        self.items.iter().find(|p| p.idx == idx)
    }

    fn first_of_type(&self, ty: &str) -> Option<&PhInfo> {
        self.items.iter().find(|p| p.ty == ty)
    }
}

/// Which master placeholder type a layout placeholder inherits from.
fn master_type_for(layout_ty: &str) -> Option<&'static str> {
    Some(match layout_ty {
        "body" | "chart" | "clipArt" | "dgm" | "media" | "obj" | "pic" | "subTitle" | "tbl" => {
            "body"
        }
        "ctrTitle" | "title" => "title",
        "dt" => "dt",
        "ftr" => "ftr",
        "sldNum" => "sldNum",
        _ => return None,
    })
}

/// Placeholder lookups for one slide: its layout, which in turn falls back to the master.
#[derive(Clone, Debug, Default)]
pub struct Inheritance {
    pub layout: Placeholders,
    pub master: Placeholders,
}

impl Inheritance {
    /// Geometry a slide placeholder with this `idx` inherits.
    pub fn geom_for(&self, idx: u32) -> Geom {
        let Some(layout_ph) = self.layout.by_idx(idx) else {
            return Geom::default();
        };
        let from_master = master_type_for(&layout_ph.ty)
            .and_then(|t| self.master.first_of_type(t))
            .map(|m| m.geom)
            .unwrap_or_default();
        layout_ph.geom.or(from_master)
    }
}

// -------------------------------------------------------------------- package

#[derive(Clone, Debug)]
pub struct Rel {
    pub ty: String,
    /// Absolute path inside the package (empty for external targets).
    pub target: String,
}

pub struct Package {
    archive: ZipArchive<Cursor<Vec<u8>>>,
}

/// Resolve a relationship `target` relative to the part that declares it.
fn resolve_target(part_path: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_string();
    }
    let mut segments: Vec<&str> = part_path.split('/').collect();
    segments.pop(); // the part's own filename
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s),
        }
    }
    segments.join("/")
}

impl Package {
    pub fn open(bytes: &[u8]) -> Result<Self, String> {
        let archive = ZipArchive::new(Cursor::new(bytes.to_vec()))
            .map_err(|e| format!("not a valid .pptx file ({e})"))?;
        Ok(Self { archive })
    }

    pub fn read_bytes(&mut self, path: &str) -> Option<Vec<u8>> {
        let mut file = self.archive.by_name(path).ok()?;
        let mut buf = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut buf).ok()?;
        Some(buf)
    }

    pub fn read_string(&mut self, path: &str) -> Option<String> {
        String::from_utf8(self.read_bytes(path)?).ok()
    }

    /// Relationships declared by `part_path`, keyed by relationship id.
    pub fn rels(&mut self, part_path: &str) -> HashMap<String, Rel> {
        let (dir, file) = part_path.rsplit_once('/').unwrap_or(("", part_path));
        let rels_path = if dir.is_empty() {
            format!("_rels/{file}.rels")
        } else {
            format!("{dir}/_rels/{file}.rels")
        };
        let Some(xml) = self.read_string(&rels_path) else {
            return HashMap::new();
        };
        let Ok(doc) = Document::parse(&xml) else {
            return HashMap::new();
        };
        doc.root_element()
            .children()
            .filter(|n| is_el(*n, NS_REL, "Relationship"))
            .filter_map(|n| {
                let id = n.attribute("Id")?.to_string();
                let ty = n.attribute("Type").unwrap_or_default().to_string();
                let external = n.attribute("TargetMode") == Some("External");
                let target = if external {
                    String::new()
                } else {
                    resolve_target(part_path, n.attribute("Target")?)
                };
                Some((id, Rel { ty, target }))
            })
            .collect()
    }

    /// Slide parts in presentation order (`sldIdLst`), falling back to numeric file order.
    pub fn slide_paths(&mut self) -> Vec<String> {
        if let Some(paths) = self.slide_paths_from_presentation()
            && !paths.is_empty()
        {
            return paths;
        }
        let mut names: Vec<(u64, String)> = self
            .archive
            .file_names()
            .filter_map(|n| {
                let num = n
                    .strip_prefix("ppt/slides/slide")?
                    .strip_suffix(".xml")?
                    .parse()
                    .ok()?;
                Some((num, n.to_string()))
            })
            .collect();
        names.sort();
        names.into_iter().map(|(_, n)| n).collect()
    }

    fn slide_paths_from_presentation(&mut self) -> Option<Vec<String>> {
        let xml = self.read_string("ppt/presentation.xml")?;
        let rels = self.rels("ppt/presentation.xml");
        let doc = Document::parse(&xml).ok()?;
        let list = child(doc.root_element(), NS_P, "sldIdLst")?;
        Some(
            list.children()
                .filter(|n| is_el(*n, NS_P, "sldId"))
                .filter_map(|n| n.attribute((NS_R, "id")))
                .filter_map(|id| rels.get(id))
                .map(|r| r.target.clone())
                .filter(|t| !t.is_empty())
                .collect(),
        )
    }

    /// Layout and master placeholder geometry for a slide part.
    pub fn inheritance_for(
        &mut self,
        slide_path: &str,
        cache: &mut HashMap<String, Placeholders>,
    ) -> Inheritance {
        let mut load = |pkg: &mut Package, path: &str| -> Placeholders {
            if let Some(hit) = cache.get(path) {
                return hit.clone();
            }
            let parsed = pkg
                .read_string(path)
                .map(|x| Placeholders::parse(&x))
                .unwrap_or_default();
            cache.insert(path.to_string(), parsed.clone());
            parsed
        };

        let slide_rels = self.rels(slide_path);
        let Some(layout_path) = slide_rels
            .values()
            .find(|r| r.ty.ends_with("/slideLayout"))
            .map(|r| r.target.clone())
        else {
            return Inheritance::default();
        };
        let layout = load(self, &layout_path);

        let master = self
            .rels(&layout_path)
            .values()
            .find(|r| r.ty.ends_with("/slideMaster"))
            .map(|r| r.target.clone())
            .map(|p| load(self, &p))
            .unwrap_or_default();

        Inheritance { layout, master }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_and_absolute_targets() {
        assert_eq!(
            resolve_target("ppt/slides/slide1.xml", "../media/image1.png"),
            "ppt/media/image1.png"
        );
        assert_eq!(
            resolve_target("ppt/presentation.xml", "slides/slide2.xml"),
            "ppt/slides/slide2.xml"
        );
        assert_eq!(
            resolve_target("ppt/slides/slide1.xml", "/ppt/media/a.png"),
            "ppt/media/a.png"
        );
        assert_eq!(resolve_target("a.xml", "b.xml"), "b.xml");
    }

    #[test]
    fn geometry_falls_back_per_property() {
        let own = Geom {
            left: Some(1),
            top: None,
            width: Some(3),
            height: None,
        };
        let base = Geom {
            left: Some(10),
            top: Some(20),
            width: Some(30),
            height: Some(40),
        };
        let merged = own.or(base);
        assert_eq!(
            merged,
            Geom {
                left: Some(1),
                top: Some(20),
                width: Some(3),
                height: Some(40)
            }
        );
    }

    #[test]
    fn layout_placeholder_inherits_from_master_by_type() {
        let xml = |body: &str| {
            format!(
                r#"<p:sldLayout xmlns:p="{NS_P}" xmlns:a="{NS_A}"><p:cSld><p:spTree>{body}</p:spTree></p:cSld></p:sldLayout>"#
            )
        };
        let ph = |idx: u32, ty: &str, xfrm: &str| {
            format!(
                r#"<p:sp><p:nvSpPr><p:cNvPr id="{idx}" name="x"/><p:cNvSpPr/><p:nvPr><p:ph type="{ty}" idx="{idx}"/></p:nvPr></p:nvSpPr><p:spPr>{xfrm}</p:spPr></p:sp>"#
            )
        };
        let layout = Placeholders::parse(&xml(&ph(1, "body", "")));
        let master = Placeholders::parse(&xml(&ph(
            7,
            "body",
            r#"<a:xfrm><a:off x="5" y="6"/><a:ext cx="7" cy="8"/></a:xfrm>"#,
        )));
        let inh = Inheritance { layout, master };

        assert_eq!(
            inh.geom_for(1),
            Geom {
                left: Some(5),
                top: Some(6),
                width: Some(7),
                height: Some(8)
            }
        );
        assert_eq!(inh.geom_for(99), Geom::default());
    }
}
