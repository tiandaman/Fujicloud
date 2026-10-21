//! XMP sidecars, so ratings and colour labels show up in Lightroom, Bridge
//! and Camera Raw. Existing sidecars (which may hold Lightroom develop
//! settings) are edited in place: only `xmp:Rating` and `xmp:Label` change.

use std::path::{Path, PathBuf};

const XMP_NS: &str = "http://ns.adobe.com/xap/1.0/";

/// Adobe's convention: `DSCF1234.RAF` → `DSCF1234.xmp`.
pub fn sidecar_path(raw: &Path) -> PathBuf {
    raw.with_extension("xmp")
}

pub fn write_sidecar(raw: &Path, rating: i32, label: Option<&str>) -> std::io::Result<()> {
    let path = sidecar_path(raw);
    let rating = rating.to_string();
    let xml = match std::fs::read_to_string(&path) {
        Ok(existing) => {
            let Some(xml) = update(&existing, &rating, label) else {
                log::warn!("leaving unrecognised sidecar alone: {}", path.display());
                return Ok(());
            };
            if xml == existing {
                return Ok(());
            }
            xml
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fresh(&rating, label),
        Err(e) => return Err(e),
    };
    let tmp = path.with_extension("xmp.tmp");
    std::fs::write(&tmp, xml)?;
    std::fs::rename(&tmp, &path)
}

fn fresh(rating: &str, label: Option<&str>) -> String {
    let label = label
        .map(|l| format!("\n   xmp:Label=\"{}\"", escape(l)))
        .unwrap_or_default();
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Fujicloud">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="{XMP_NS}"
   xmp:Rating="{rating}"{label}/>
 </rdf:RDF>
</x:xmpmeta>
"#
    )
}

/// Returns `None` if the document has no `rdf:Description` to attach to.
pub fn update(xml: &str, rating: &str, label: Option<&str>) -> Option<String> {
    let mut xml = xml.to_owned();
    if !xml.contains("<rdf:Description") {
        return None;
    }
    set_property(&mut xml, "xmp:Rating", Some(rating));
    set_property(&mut xml, "xmp:Label", label.map(escape).as_deref());
    if xml.contains("xmp:") && !xml.contains(&format!("xmlns:xmp=\"{XMP_NS}\"")) {
        insert_attr(&mut xml, &format!("xmlns:xmp=\"{XMP_NS}\""));
    }
    Some(xml)
}

/// Sets (or with `None`, removes) a simple property written either as an
/// attribute (`xmp:Rating="3"`) or as an element (`<xmp:Rating>3</xmp:Rating>`).
fn set_property(xml: &mut String, name: &str, value: Option<&str>) {
    let attr = format!("{name}=\"");
    if let Some(start) = find_attr(xml, &attr) {
        let value_start = start + attr.len();
        let Some(len) = xml[value_start..].find('"') else {
            return;
        };
        match value {
            Some(v) => xml.replace_range(value_start..value_start + len, v),
            None => {
                // Also swallow the whitespace before the attribute.
                let ws = xml[..start].len() - xml[..start].trim_end().len();
                xml.replace_range(start - ws..value_start + len + 1, "");
            }
        }
        return;
    }
    let (open, close) = (format!("<{name}>"), format!("</{name}>"));
    if let (Some(s), Some(e)) = (xml.find(&open), xml.find(&close)) {
        match value {
            Some(v) => xml.replace_range(s + open.len()..e, v),
            None => xml.replace_range(s..e + close.len(), ""),
        }
        return;
    }
    if let Some(v) = value {
        insert_attr(xml, &format!("{name}=\"{v}\""));
    }
}

/// Matches `name="` only at an attribute boundary (so `xmp:Rating` does not
/// match inside `MicrosoftPhoto:xmp:Rating` or similar).
fn find_attr(xml: &str, attr: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = xml[from..].find(attr) {
        let at = from + i;
        if at == 0 || xml[..at].ends_with(char::is_whitespace) {
            return Some(at);
        }
        from = at + attr.len();
    }
    None
}

fn insert_attr(xml: &mut String, attr: &str) {
    if let Some(i) = xml.find("<rdf:Description") {
        xml.insert_str(i + "<rdf:Description".len(), &format!("\n   {attr}"));
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIGHTROOM: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:Exposure2012="+0.35"
   crs:Temperature="5600">
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    #[test]
    fn preserves_develop_settings() {
        let out = update(LIGHTROOM, "4", Some("Red")).unwrap();
        assert!(out.contains(r#"crs:Exposure2012="+0.35""#));
        assert!(out.contains(r#"xmp:Rating="4""#));
        assert!(out.contains(r#"xmp:Label="Red""#));
        assert!(out.contains(r#"xmlns:xmp="http://ns.adobe.com/xap/1.0/""#));

        let again = update(&out, "2", None).unwrap();
        assert!(again.contains(r#"xmp:Rating="2""#));
        assert!(!again.contains("xmp:Label"));
        assert_eq!(again.matches("xmlns:xmp=").count(), 1);
    }

    #[test]
    fn element_form() {
        let xml = r#"<rdf:Description rdf:about=""><xmp:Rating>1</xmp:Rating></rdf:Description>"#;
        let out = update(xml, "5", None).unwrap();
        assert!(out.contains("<xmp:Rating>5</xmp:Rating>"));
    }

    #[test]
    fn writes_fresh_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("DSCF0001.RAF");
        write_sidecar(&raw, 3, Some("Green")).unwrap();
        let xml = std::fs::read_to_string(dir.path().join("DSCF0001.xmp")).unwrap();
        assert!(xml.contains(r#"xmp:Rating="3""#) && xml.contains(r#"xmp:Label="Green""#));
        assert!(update(&xml, "3", Some("Green")).unwrap() == xml);
    }

    #[test]
    fn unknown_documents_are_untouched() {
        assert_eq!(update("<nope/>", "1", None), None);
    }
}
