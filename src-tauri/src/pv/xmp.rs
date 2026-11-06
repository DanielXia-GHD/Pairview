//! Pairview: reading and editing XMP sidecars as text (docs/IMPLEMENTATION_PLAN.md M1-08).
//!
//! Edits are minimal: only the properties being changed are touched, so everything else in the
//! file (Lightroom develop settings, keywords, other tools' namespaces) stays byte for byte.
//! Properties are written as attributes of the first `rdf:Description`, which is how Lightroom
//! writes them. A file without an `rdf:Description` is refused, never replaced.
//!
//! Properties:
//! - `xmp:Rating`: 0–5, or -1 for a rejected Photo (Adobe convention, also RapidRAW's).
//! - `xmp:Label`: the color label (`Red`, `Yellow`, …) or any text another tool wrote.
//! - `pairview:Flag`: `pick` / `reject`. Adobe XMP has no pick flag.
//! - `pairview:KeepRaw`: `keep` / `drop`, the owner's Keep-RAW lock (PRD §6.5). `auto` = absent.
//! - `pairview:Edited`: when the display copy was rendered from the RAW (M3-02), so a rebuilt
//!   catalog still knows the Photo is `✎`.

use anyhow::{Result, bail};
use quick_xml::escape::{escape, unescape};
use regex::Regex;

pub const XMP_NS: &str = "http://ns.adobe.com/xap/1.0/";
pub const PAIRVIEW_NS: &str = "http://ns.pairview.app/1.0/";

/// The values Pairview reads from a sidecar. `None` = the property isn't there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct XmpFields {
    pub rating: Option<i64>,
    pub label: Option<String>,
    pub flag: Option<String>,
    pub keep_raw: Option<String>,
    /// When the display copy was last rendered from the RAW (Unix ms). Absent = camera original.
    pub edited: Option<String>,
}

/// One property change: `Some(None)` removes it, `None` leaves it alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct XmpEdit {
    pub rating: Option<Option<i64>>,
    pub label: Option<Option<String>>,
    pub flag: Option<Option<String>>,
    pub keep_raw: Option<Option<String>>,
    pub edited: Option<Option<String>>,
}

impl XmpEdit {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

pub fn read(content: &str) -> XmpFields {
    let xmp = prefix_for(content, XMP_NS).unwrap_or_else(|| "xmp".into());
    let pv = prefix_for(content, PAIRVIEW_NS).unwrap_or_else(|| "pairview".into());
    XmpFields {
        rating: property(content, &xmp, "Rating").and_then(|v| v.trim().parse().ok()),
        label: property(content, &xmp, "Label").filter(|v| !v.is_empty()),
        flag: property(content, &pv, "Flag").filter(|v| !v.is_empty()),
        keep_raw: property(content, &pv, "KeepRaw").filter(|v| !v.is_empty()),
        edited: property(content, &pv, "Edited").filter(|v| !v.is_empty()),
    }
}

/// Applies `edit` to an existing sidecar, or to a new one when `existing` is `None`.
pub fn apply(existing: Option<&str>, edit: &XmpEdit) -> Result<String> {
    let mut content = match existing {
        Some(c) => c.to_string(),
        None => SKELETON.to_string(),
    };
    if edit.is_empty() {
        return Ok(content);
    }
    let Some(desc) = first_description(&content) else {
        bail!("not an XMP sidecar Pairview understands (no rdf:Description)");
    };

    let xmp = scoped_prefix(&content, desc.tag_end, XMP_NS);
    let pv = scoped_prefix(&content, desc.tag_end, PAIRVIEW_NS);
    let changes = [
        (
            &xmp,
            XMP_NS,
            "Rating",
            edit.rating.as_ref().map(|r| r.map(|v| v.to_string())),
        ),
        (&xmp, XMP_NS, "Label", edit.label.clone()),
        (&pv, PAIRVIEW_NS, "Flag", edit.flag.clone()),
        (&pv, PAIRVIEW_NS, "KeepRaw", edit.keep_raw.clone()),
        (&pv, PAIRVIEW_NS, "Edited", edit.edited.clone()),
    ];

    for (prefix, ns, name, change) in changes {
        let Some(value) = change else {
            continue;
        };
        if let Some(value) = &value
            && let Some(replaced) = replace_attribute(&content, &prefix.name, name, value)
        {
            content = replaced;
            continue;
        }
        content = remove_property(&content, &prefix.name, name);
        if let Some(value) = value {
            let desc = first_description(&content).expect("still present after removals");
            let mut attrs = String::new();
            if !prefix.declared && !declared_in_scope(&content, desc.tag_end, &prefix.name) {
                attrs.push_str(&format!("\n    xmlns:{}=\"{}\"", prefix.name, ns));
            }
            attrs.push_str(&format!(
                "\n    {}:{}=\"{}\"",
                prefix.name,
                name,
                escape(&value)
            ));
            content.insert_str(desc.insert_at, &attrs);
        }
    }
    Ok(content)
}

const SKELETON: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Pairview">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""/>
 </rdf:RDF>
</x:xmpmeta>
"#;

struct Description {
    /// Index just past the start tag's `>`.
    tag_end: usize,
    /// Where to insert attributes: before `>` or `/>`.
    insert_at: usize,
}

fn first_description(content: &str) -> Option<Description> {
    let start = content.find("<rdf:Description")?;
    let after_name = start + "<rdf:Description".len();
    // The name must end here (not `<rdf:DescriptionFoo`).
    if !content[after_name..].starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/') {
        return None;
    }
    let mut quote = None;
    for (i, c) in content[after_name..].char_indices() {
        let at = after_name + i;
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => {
                let self_closing = content[..at].ends_with('/');
                return Some(Description {
                    tag_end: at + 1,
                    insert_at: if self_closing { at - 1 } else { at },
                });
            }
            _ => {}
        }
    }
    None
}

struct Prefix {
    name: String,
    declared: bool,
}

/// The prefix bound to `ns` in scope of the first `rdf:Description` (its own start tag and its
/// ancestors, which all come before `tag_end`), or the conventional one, to be declared.
fn scoped_prefix(content: &str, tag_end: usize, ns: &str) -> Prefix {
    match prefix_for(&content[..tag_end], ns) {
        Some(name) => Prefix {
            name,
            declared: true,
        },
        None => Prefix {
            name: if ns == XMP_NS { "xmp" } else { "pairview" }.into(),
            declared: false,
        },
    }
}

fn prefix_for(content: &str, ns: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#"xmlns:([A-Za-z_][\w.-]*)\s*=\s*["']{}["']"#,
        regex::escape(ns)
    ))
    .expect("valid regex");
    re.captures(content).map(|c| c[1].to_string())
}

fn declared_in_scope(content: &str, tag_end: usize, prefix: &str) -> bool {
    content[..tag_end].contains(&format!("xmlns:{prefix}="))
}

fn property(content: &str, prefix: &str, name: &str) -> Option<String> {
    let p = regex::escape(&format!("{prefix}:{name}"));
    let attr = Regex::new(&format!(r#"\s{p}\s*=\s*(?:"([^"]*)"|'([^']*)')"#)).expect("regex");
    let elem = Regex::new(&format!(r#"<{p}(?:\s[^>]*)?>([^<]*)</{p}>"#)).expect("regex");
    let raw = attr
        .captures(content)
        .map(|c| c.get(1).or(c.get(2)).map_or("", |m| m.as_str()).to_string())
        .or_else(|| elem.captures(content).map(|c| c[1].to_string()))?;
    Some(
        unescape(&raw)
            .map_or(raw.clone(), |v| v.into_owned())
            .trim()
            .to_string(),
    )
}

/// Changes the value of a property written exactly once, as an attribute, where it stands.
fn replace_attribute(content: &str, prefix: &str, name: &str, value: &str) -> Option<String> {
    let p = regex::escape(&format!("{prefix}:{name}"));
    let attr = Regex::new(&format!(r#"(\s{p}\s*=\s*)(?:"[^"]*"|'[^']*')"#)).expect("regex");
    let elem = Regex::new(&format!(r#"<{p}[\s>/]"#)).expect("regex");
    if attr.find_iter(content).count() != 1 || elem.is_match(content) {
        return None;
    }
    let value = escape(value);
    Some(
        attr.replace(content, |c: &regex::Captures| {
            format!("{}\"{}\"", &c[1], value)
        })
        .into_owned(),
    )
}

fn remove_property(content: &str, prefix: &str, name: &str) -> String {
    let p = regex::escape(&format!("{prefix}:{name}"));
    let attr = Regex::new(&format!(r#"\s+{p}\s*=\s*(?:"[^"]*"|'[^']*')"#)).expect("regex");
    let elem = Regex::new(&format!(
        r#"[ \t]*<{p}(?:\s[^>]*)?>[^<]*</{p}>[ \t]*\r?\n?"#
    ))
    .expect("regex");
    let empty = Regex::new(&format!(r#"[ \t]*<{p}\s*/>[ \t]*\r?\n?"#)).expect("regex");
    let content = attr.replace_all(content, "");
    let content = elem.replace_all(&content, "");
    empty.replace_all(&content, "").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like a Lightroom Classic sidecar for a Sony ARW.
    const LIGHTROOM: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000 1.000000, 0000/00/00-00:00:00        ">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:tiff="http://ns.adobe.com/tiff/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
   xmp:ModifyDate="2026-10-25T10:12:00+09:00"
   xmp:Rating="3"
   xmp:Label="Red"
   tiff:Make="SONY"
   crs:Version="17.0"
   crs:Exposure2012="+0.35"
   crs:HasSettings="True">
   <dc:subject>
    <rdf:Bag>
     <rdf:li>Tokyo &amp; night</rdf:li>
    </rdf:Bag>
   </dc:subject>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;

    fn well_formed(xml: &str) {
        let mut reader = quick_xml::Reader::from_str(xml);
        loop {
            match reader.read_event() {
                Ok(quick_xml::events::Event::Eof) => break,
                Ok(_) => {}
                Err(e) => panic!("not well-formed: {e}\n{xml}"),
            }
        }
    }

    #[test]
    fn reads_lightroom_attributes() {
        assert_eq!(
            read(LIGHTROOM),
            XmpFields {
                rating: Some(3),
                label: Some("Red".into()),
                flag: None,
                keep_raw: None,
                edited: None
            }
        );
    }

    #[test]
    fn changes_only_what_was_asked_and_keeps_everything_else() {
        let edit = XmpEdit {
            rating: Some(Some(5)),
            ..Default::default()
        };
        let out = apply(Some(LIGHTROOM), &edit).unwrap();
        well_formed(&out);
        assert_eq!(read(&out).rating, Some(5));
        assert_eq!(read(&out).label.as_deref(), Some("Red"));
        // Everything except the rating line is identical.
        let strip = |s: &str| -> Vec<String> {
            s.lines()
                .filter(|l| !l.contains("Rating"))
                .map(|l| l.trim_end().to_string())
                .collect()
        };
        assert_eq!(strip(&out), strip(LIGHTROOM));
        assert!(out.contains("crs:Exposure2012=\"+0.35\""));
        assert!(out.contains("<rdf:li>Tokyo &amp; night</rdf:li>"));
    }

    #[test]
    fn adds_the_pairview_namespace_once_and_removes_cleanly() {
        let edit = XmpEdit {
            flag: Some(Some("pick".into())),
            keep_raw: Some(Some("keep".into())),
            ..Default::default()
        };
        let out = apply(Some(LIGHTROOM), &edit).unwrap();
        well_formed(&out);
        assert_eq!(out.matches(PAIRVIEW_NS).count(), 1);
        let fields = read(&out);
        assert_eq!(
            (fields.flag.as_deref(), fields.keep_raw.as_deref()),
            (Some("pick"), Some("keep"))
        );

        let cleared = apply(
            Some(&out),
            &XmpEdit {
                flag: Some(None),
                keep_raw: Some(None),
                label: Some(None),
                ..Default::default()
            },
        )
        .unwrap();
        well_formed(&cleared);
        let fields = read(&cleared);
        assert_eq!(
            (fields.flag, fields.keep_raw, fields.label),
            (None, None, None)
        );
        assert_eq!(fields.rating, Some(3));
    }

    #[test]
    fn handles_element_form_and_other_prefixes() {
        let capture_one_like = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:xap="http://ns.adobe.com/xap/1.0/">
  <rdf:Description rdf:about="">
   <xap:Rating>2</xap:Rating>
   <xap:Label>Green</xap:Label>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;
        assert_eq!(read(capture_one_like).rating, Some(2));
        let out = apply(
            Some(capture_one_like),
            &XmpEdit {
                rating: Some(Some(-1)),
                ..Default::default()
            },
        )
        .unwrap();
        well_formed(&out);
        assert_eq!(out.matches("Rating").count(), 1);
        assert!(out.contains("xap:Rating=\"-1\""));
        assert!(!out.contains("xmlns:xmp="));
        assert_eq!(read(&out).label.as_deref(), Some("Green"));
    }

    #[test]
    fn creates_a_new_sidecar_and_escapes_values() {
        let out = apply(
            None,
            &XmpEdit {
                rating: Some(Some(4)),
                label: Some(Some("To <check> & \"keep\"".into())),
                ..Default::default()
            },
        )
        .unwrap();
        well_formed(&out);
        let fields = read(&out);
        assert_eq!(fields.rating, Some(4));
        assert_eq!(fields.label.as_deref(), Some("To <check> & \"keep\""));
    }

    #[test]
    fn refuses_files_it_does_not_understand() {
        for bad in ["", "not xml", "<x:xmpmeta/>", "<rdf:DescriptionX>"] {
            let edit = XmpEdit {
                rating: Some(Some(1)),
                ..Default::default()
            };
            assert!(apply(Some(bad), &edit).is_err(), "{bad:?}");
        }
        // An empty edit never fails and never changes anything.
        assert_eq!(
            apply(Some("not xml"), &XmpEdit::default()).unwrap(),
            "not xml"
        );
    }
}
