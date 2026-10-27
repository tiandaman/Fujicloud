//! XMP sidecars, so ratings and colour labels show up in Lightroom, Bridge
//! and Camera Raw, and everything Fujicloud records about a photo (picks,
//! rejects, favourites, albums) travels with it to other laptops. Existing
//! sidecars (which may hold Lightroom develop settings) are edited in place:
//! only the properties below change.

use std::path::{Path, PathBuf};

use crate::Kind;

const XMP_NS: &str = "http://ns.adobe.com/xap/1.0/";
const OUR_NS: &str = "https://fujicloud.app/ns/1.0/";

/// What a sidecar records about a photo.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tags {
    pub rating: i32,
    pub label: Option<String>,
    /// `1` pick, `-1` reject.
    pub flag: i32,
    pub favorite: bool,
    pub albums: Vec<String>,
    /// The faces found in it, and the pets: so another PC needn't look
    /// again, and the names given here are known there.
    pub faces: Option<Found>,
    pub pets: Option<Found>,
    /// Its smart search description: the model's name, and the vector as
    /// stored (quantized).
    pub smart: Option<(String, Vec<u8>)>,
}

impl Tags {
    /// Nothing worth creating a sidecar for.
    pub fn is_empty(&self) -> bool {
        *self == Tags::default()
    }
}

/// What a model found in a photo (possibly nothing).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    pub model: String,
    pub items: Vec<Seen>,
}

/// A face or pet: where (fractions of the picture: x, y, width, height),
/// how sure the model was, its description, and who it is if known.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub bbox: [f32; 4],
    pub score: f32,
    pub embedding: Vec<f32>,
    pub name: Option<String>,
    /// Someone said so, rather than it being grouped there.
    pub confirmed: bool,
}

/// `model;x,y,w,h,score,confirmed,description,name;...`: the description as
/// base64 half floats, the name percent-escaped.
fn found_to_text(found: &Found) -> String {
    use base64::Engine;
    let mut out = found.model.replace(';', "");
    for s in &found.items {
        let bytes: Vec<u8> = s
            .embedding
            .iter()
            .flat_map(|x| half::f16::from_f32(*x).to_le_bytes())
            .collect();
        let [x, y, w, h] = s.bbox;
        out.push_str(&format!(
            ";{x:.4},{y:.4},{w:.4},{h:.4},{:.3},{},{},{}",
            s.score,
            s.confirmed as u8,
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes),
            s.name.as_deref().map(percent).unwrap_or_default()
        ));
    }
    escape(&out)
}

fn found_from_text(text: &str) -> Option<Found> {
    use base64::Engine;
    let text = unescape(text);
    let mut parts = text.split(';');
    let model = parts.next()?.trim().to_owned();
    if model.is_empty() {
        return None;
    }
    let mut items = Vec::new();
    for part in parts {
        let f: Vec<&str> = part.split(',').collect();
        let [x, y, w, h, score, confirmed, description, name] = f[..] else {
            return None;
        };
        let num = |s: &str| s.trim().parse::<f32>().ok().filter(|v| v.is_finite());
        let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(description.trim())
            .ok()?;
        items.push(Seen {
            bbox: [num(x)?, num(y)?, num(w)?, num(h)?],
            score: num(score)?,
            embedding: bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| half::f16::from_le_bytes(*b).to_f32())
                .collect(),
            name: Some(unpercent(name)).filter(|n| !n.trim().is_empty()),
            confirmed: confirmed.trim() == "1",
        });
    }
    Some(Found { model, items })
}

fn smart_to_text((model, vector): &(String, Vec<u8>)) -> String {
    use base64::Engine;
    escape(&format!(
        "{};{}",
        model.replace(';', ""),
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(vector)
    ))
}

fn smart_from_text(text: &str) -> Option<(String, Vec<u8>)> {
    use base64::Engine;
    let text = unescape(text);
    let (model, vector) = text.split_once(';')?;
    let vector = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(vector.trim())
        .ok()?;
    (!model.trim().is_empty() && !vector.is_empty()).then(|| (model.trim().to_owned(), vector))
}

/// Names may hold anything; the list's separators are escaped.
fn percent(s: &str) -> String {
    s.replace('%', "%25")
        .replace(';', "%3B")
        .replace(',', "%2C")
}

fn unpercent(s: &str) -> String {
    s.replace("%2C", ",")
        .replace("%3B", ";")
        .replace("%25", "%")
}

/// Adobe's convention for camera files: `DSCF1234.RAF` → `DSCF1234.xmp`,
/// shared by a RAW+JPEG pair. Edits get their own (`DSCF1234.psd.xmp`) so
/// they don't share one with the original they were made from.
pub fn sidecar_path(file: &Path, kind: Kind) -> PathBuf {
    if kind.is_edit_format() {
        let mut name = file.as_os_str().to_owned();
        name.push(".xmp");
        PathBuf::from(name)
    } else {
        file.with_extension("xmp")
    }
}

/// Saves `tags` to the photo's sidecar, keeping everything else in it. No
/// sidecar is created for a photo with nothing to record.
pub fn write(file: &Path, kind: Kind, tags: &Tags) -> std::io::Result<()> {
    let path = sidecar_path(file, kind);
    let xml = match std::fs::read_to_string(&path) {
        Ok(existing) => {
            let Some(xml) = update(&existing, tags) else {
                log::warn!("leaving unrecognised sidecar alone: {}", path.display());
                return Ok(());
            };
            if xml == existing {
                return Ok(());
            }
            xml
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if tags.is_empty() {
                return Ok(());
            }
            update(FRESH, tags).expect("the template has a description")
        }
        Err(e) => return Err(e),
    };
    let tmp = path.with_extension("xmp.tmp");
    std::fs::write(&tmp, xml)?;
    std::fs::rename(&tmp, &path)
}

/// Reads what a sidecar records. Lightroom's "rejected" (`xmp:Rating="-1"`)
/// counts as a reject.
pub fn read(path: &Path) -> std::io::Result<Tags> {
    Ok(parse(&std::fs::read_to_string(path)?))
}

pub fn parse(xml: &str) -> Tags {
    let rating: i32 = get_property(xml, "xmp:Rating")
        .and_then(|v| v.trim().parse::<f32>().ok())
        .map_or(0, |r| r.round() as i32);
    let flag = get_property(xml, "fujicloud:Flag")
        .and_then(|v| v.trim().parse::<i32>().ok())
        .unwrap_or(if rating < 0 { -1 } else { 0 })
        .clamp(-1, 1);
    Tags {
        rating: rating.clamp(0, 5),
        label: get_property(xml, "xmp:Label")
            .map(unescape)
            .filter(|l| !l.is_empty()),
        flag,
        favorite: get_property(xml, "fujicloud:Favorite").is_some_and(|v| v.trim() == "1"),
        albums: get_property(xml, "fujicloud:Albums")
            .map(|v| {
                v.split(';')
                    .map(|a| unescape(&a.replace("%3B", ";").replace("%25", "%")))
                    .filter(|a| !a.trim().is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        faces: get_property(xml, "fujicloud:Faces").and_then(found_from_text),
        pets: get_property(xml, "fujicloud:Pets").and_then(found_from_text),
        smart: get_property(xml, "fujicloud:Smart").and_then(smart_from_text),
    }
}

const FRESH: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Fujicloud">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""/>
 </rdf:RDF>
</x:xmpmeta>
"#;

/// Returns `None` if the document has no `rdf:Description` to attach to.
pub fn update(xml: &str, tags: &Tags) -> Option<String> {
    let mut xml = xml.to_owned();
    if !xml.contains("<rdf:Description") {
        return None;
    }
    let albums = tags
        .albums
        .iter()
        .map(|a| escape(&a.replace('%', "%25").replace(';', "%3B")))
        .collect::<Vec<_>>()
        .join(";");
    set_property(&mut xml, "xmp:Rating", Some(&tags.rating.to_string()));
    set_property(
        &mut xml,
        "xmp:Label",
        tags.label.as_deref().map(escape).as_deref(),
    );
    set_property(
        &mut xml,
        "fujicloud:Flag",
        (tags.flag != 0).then(|| tags.flag.to_string()).as_deref(),
    );
    set_property(&mut xml, "fujicloud:Favorite", tags.favorite.then_some("1"));
    set_property(
        &mut xml,
        "fujicloud:Albums",
        (!albums.is_empty()).then_some(albums.as_str()),
    );
    // Only ever added or replaced: a PC that hasn't looked at the photo
    // saving its rating mustn't drop what another found.
    if let Some(found) = &tags.faces {
        set_property(&mut xml, "fujicloud:Faces", Some(&found_to_text(found)));
    }
    if let Some(found) = &tags.pets {
        set_property(&mut xml, "fujicloud:Pets", Some(&found_to_text(found)));
    }
    if let Some(smart) = &tags.smart {
        set_property(&mut xml, "fujicloud:Smart", Some(&smart_to_text(smart)));
    }
    for (prefix, ns) in [("xmp", XMP_NS), ("fujicloud", OUR_NS)] {
        let used = xml.contains(&format!("{prefix}:"));
        if used && !xml.contains(&format!("xmlns:{prefix}=\"{ns}\"")) {
            insert_attr(&mut xml, &format!("xmlns:{prefix}=\"{ns}\""));
        }
    }
    Some(xml)
}

/// A simple property's value, written as an attribute or as an element.
fn get_property<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let attr = format!("{name}=\"");
    if let Some(start) = find_attr(xml, &attr) {
        let from = start + attr.len();
        return xml[from..].find('"').map(|len| &xml[from..from + len]);
    }
    let (open, close) = (format!("<{name}>"), format!("</{name}>"));
    let s = xml.find(&open)? + open.len();
    let e = xml[s..].find(&close)? + s;
    Some(&xml[s..e])
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

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
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

    fn tags(rating: i32, label: Option<&str>) -> Tags {
        Tags {
            rating,
            label: label.map(str::to_owned),
            ..Tags::default()
        }
    }

    #[test]
    fn preserves_develop_settings() {
        let out = update(LIGHTROOM, &tags(4, Some("Red"))).unwrap();
        assert!(out.contains(r#"crs:Exposure2012="+0.35""#));
        assert!(out.contains(r#"xmp:Rating="4""#));
        assert!(out.contains(r#"xmp:Label="Red""#));
        assert!(out.contains(r#"xmlns:xmp="http://ns.adobe.com/xap/1.0/""#));
        assert!(!out.contains("fujicloud"), "nothing of ours to record");
        let again = update(&out, &tags(2, None)).unwrap();
        assert!(again.contains(r#"xmp:Rating="2""#));
        assert!(!again.contains("xmp:Label"));
        assert_eq!(again.matches("xmlns:xmp=").count(), 1);
    }

    #[test]
    fn element_form() {
        let xml = r#"<rdf:Description rdf:about=""><xmp:Rating>1</xmp:Rating></rdf:Description>"#;
        assert_eq!(parse(xml).rating, 1);
        let out = update(xml, &tags(5, None)).unwrap();
        assert!(out.contains("<xmp:Rating>5</xmp:Rating>"));
    }

    #[test]
    fn everything_round_trips() {
        let all = Tags {
            rating: 3,
            label: Some("Green & \"Blue\"".into()),
            flag: 1,
            favorite: true,
            albums: vec!["Halifax 2026".into(), "Best; of 50%".into()],
            ..Tags::default()
        };
        let xml = update(LIGHTROOM, &all).unwrap();
        assert!(xml.contains(r#"xmlns:fujicloud="https://fujicloud.app/ns/1.0/""#));
        assert_eq!(parse(&xml), all);
        // Clearing them removes our properties again.
        let cleared = update(&xml, &Tags::default()).unwrap();
        assert_eq!(parse(&cleared), Tags::default());
        assert!(!cleared.contains("fujicloud:"));
        assert!(cleared.contains(r#"crs:Temperature="5600""#));
    }

    #[test]
    fn lightroom_reject_counts_as_reject() {
        let xml = r#"<rdf:Description rdf:about="" xmp:Rating="-1"/>"#;
        assert_eq!(
            parse(xml),
            Tags {
                flag: -1,
                ..Tags::default()
            }
        );
    }

    #[test]
    fn writes_fresh_sidecars_only_when_needed() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("DSCF0001.RAF");
        write(&raw, Kind::Raf, &Tags::default()).unwrap();
        assert!(!dir.path().join("DSCF0001.xmp").exists());
        write(&raw, Kind::Raf, &tags(3, Some("Green"))).unwrap();
        assert_eq!(
            read(&dir.path().join("DSCF0001.xmp")).unwrap(),
            tags(3, Some("Green"))
        );
    }

    #[test]
    fn edits_get_their_own_sidecar() {
        let psd = Path::new("/lib/DSCF0001.psd");
        assert_eq!(
            sidecar_path(psd, Kind::Psd),
            Path::new("/lib/DSCF0001.psd.xmp")
        );
        assert_eq!(
            sidecar_path(Path::new("/lib/DSCF0001.JPG"), Kind::Jpeg),
            Path::new("/lib/DSCF0001.xmp")
        );
    }

    #[test]
    fn what_was_found_travels_and_stays() {
        let found = Found {
            model: "scrfd-10g+arcface-w600k-r50".into(),
            items: vec![
                Seen {
                    bbox: [0.1, 0.2, 0.3, 0.25],
                    score: 0.91,
                    embedding: vec![0.5, -0.25, 0.125, 0.0],
                    name: Some("Ana; \"Nana\", <3 %".into()),
                    confirmed: true,
                },
                Seen {
                    bbox: [0.6, 0.1, 0.1, 0.1],
                    score: 0.8,
                    embedding: vec![0.0, 1.0, 0.0, -1.0],
                    name: None,
                    confirmed: false,
                },
            ],
        };
        let mine = Tags {
            rating: 3,
            faces: Some(found.clone()),
            pets: Some(Found {
                model: "yolox+dinov2".into(),
                items: vec![],
            }),
            smart: Some(("siglip".into(), vec![1, 2, 3, 250])),
            ..Tags::default()
        };
        let xml = update(LIGHTROOM, &mine).unwrap();
        assert_eq!(parse(&xml), mine, "{xml}");
        assert!(xml.contains(r#"crs:Exposure2012="+0.35""#));
        // Another PC that found nothing yet saves a rating: nothing is lost.
        let again = update(&xml, &tags(5, None)).unwrap();
        let read = parse(&again);
        assert_eq!(read.rating, 5);
        assert_eq!(
            (read.faces, read.pets, read.smart),
            (mine.faces, mine.pets, mine.smart)
        );
        // A damaged list is ignored, not half read.
        assert_eq!(found_from_text("m;0.1,0.2"), None);
    }

    #[test]
    fn unknown_documents_are_untouched() {
        assert_eq!(update("<nope/>", &tags(1, None)), None);
    }
}
