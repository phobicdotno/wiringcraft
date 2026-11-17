//! Parts from BMEcat catalogues (XML, versions 1.2 and 2005).
//!
//! A catalogue has a `HEADER` (supplier, default language) and products: `PRODUCT` (2005) or
//! `ARTICLE` (1.2) inside `T_NEW_CATALOG`, `T_UPDATE_PRODUCTS` or `T_UPDATE_PRICES`. Each one
//! carries the supplier's order number (`SUPPLIER_PID` / `SUPPLIER_AID`), details
//! (`DESCRIPTION_SHORT`, `MANUFACTURER_NAME`, `MANUFACTURER_PID` / `MANUFACTURER_AID`,
//! `INTERNATIONAL_PID` / `EAN`) and feature blocks (`PRODUCT_FEATURES` / `ARTICLE_FEATURES`)
//! that name a classification such as ECLASS with its class and features (`FNAME`, `FVALUE`,
//! `FUNIT`). Written from the published BMEcat specification; prices are not read.

use std::collections::BTreeMap;

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use wc_core::Part;

use crate::{Error, Result};

/// Property holding the maker's own part number when it differs from the order number.
pub const PROP_MANUFACTURER_PID: &str = "ManufacturerPartNumber";
/// Property holding the GTIN/EAN.
pub const PROP_GTIN: &str = "GTIN";
/// Property holding the classification and class, e.g. `ECLASS-11.0: 27141120`.
pub const PROP_CLASS: &str = "Classification";

/// Deepest nesting accepted; real catalogues stay under fifteen.
const MAX_DEPTH: usize = 64;

#[derive(Default)]
struct Product {
    supplier_pid: String,
    manufacturer: String,
    manufacturer_pid: String,
    gtin: String,
    /// (language, text)
    short: Vec<(String, String)>,
    long: Vec<(String, String)>,
    mode: String,
    classes: Vec<String>,
    /// Feature system name of the block being read.
    system: String,
    features: Vec<(String, String)>,
    /// The feature being read: name, values, unit.
    fname: String,
    fvalues: Vec<String>,
    funit: String,
}

/// Pick the text in the catalogue's language, then English, then the first one.
fn pick<'a>(texts: &'a [(String, String)], lang: &str) -> Option<&'a str> {
    texts
        .iter()
        .find(|(l, _)| !lang.is_empty() && l == lang)
        .or_else(|| texts.iter().find(|(l, _)| matches!(l.as_str(), "eng" | "en")))
        .or_else(|| texts.first())
        .map(|(_, t)| t.as_str())
}

/// Read a BMEcat catalogue into parts. Returns the parts (sorted by id) and warnings.
pub fn parts(text: &str) -> Result<(Vec<Part>, Vec<String>)> {
    let mut reader = Reader::from_str(text);
    let mut path: Vec<String> = Vec::new();
    let mut seen_root = false;
    let mut supplier_name = String::new();
    let mut language = String::new();
    let mut product: Option<Product> = None;
    let mut out: BTreeMap<String, Part> = BTreeMap::new();
    let mut warnings = Vec::new();
    let mut count = 0usize;
    let mut deleted = 0usize;
    let mut lang_attr = String::new();
    let mut text_buf = String::new();

    loop {
        let ev = reader
            .read_event()
            .map_err(|e| Error::Format(format!("not readable XML: {e}")))?;
        match ev {
            Event::Start(_) | Event::Empty(_) if path.len() >= MAX_DEPTH => {
                return Err(Error::Format("the file is nested too deeply".into()));
            }
            Event::Start(e) => {
                let tag = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if path.is_empty() {
                    if tag != "BMECAT" {
                        return Err(Error::Format("not a BMEcat catalogue (no BMECAT root element)".into()));
                    }
                    seen_root = true;
                }
                let attr = |name: &str| {
                    e.attributes()
                        .flatten()
                        .find(|a| a.key.local_name().as_ref() == name.as_bytes())
                        .and_then(|a| a.unescape_value().ok())
                        .map(|v| v.trim().to_owned())
                        .unwrap_or_default()
                };
                if matches!(tag.as_str(), "PRODUCT" | "ARTICLE") && product.is_none() {
                    product = Some(Product {
                        mode: attr("mode"),
                        ..Default::default()
                    });
                }
                lang_attr = attr("lang");
                text_buf.clear();
                path.push(tag);
            }
            Event::Empty(e) => {
                if path.is_empty() {
                    if e.local_name().as_ref() != b"BMECAT" {
                        return Err(Error::Format("not a BMEcat catalogue (no BMECAT root element)".into()));
                    }
                    seen_root = true;
                }
            }
            Event::Text(t) => {
                if let Ok(v) = t.unescape() {
                    text_buf.push_str(&v);
                }
            }
            Event::CData(t) => text_buf.push_str(&String::from_utf8_lossy(&t)),
            Event::End(_) => {
                let Some(tag) = path.pop() else {
                    return Err(Error::Format("unbalanced end tag".into()));
                };
                let v = text_buf.trim().to_owned();
                text_buf.clear();
                let parent = path.last().map(String::as_str).unwrap_or("");
                if let Some(p) = product.as_mut() {
                    match tag.as_str() {
                        "SUPPLIER_PID" | "SUPPLIER_AID" if matches!(parent, "PRODUCT" | "ARTICLE") => {
                            p.supplier_pid = v
                        }
                        "MANUFACTURER_NAME" => p.manufacturer = v,
                        "MANUFACTURER_PID" | "MANUFACTURER_AID" => p.manufacturer_pid = v,
                        "INTERNATIONAL_PID" | "EAN" if p.gtin.is_empty() && !v.is_empty() => p.gtin = v,
                        "DESCRIPTION_SHORT" if !v.is_empty() => p.short.push((lang_attr.clone(), v)),
                        "DESCRIPTION_LONG" if !v.is_empty() => p.long.push((lang_attr.clone(), v)),
                        "REFERENCE_FEATURE_SYSTEM_NAME" => p.system = v,
                        "REFERENCE_FEATURE_GROUP_ID" | "REFERENCE_FEATURE_GROUP_NAME" if !v.is_empty() => {
                            let class = if p.system.is_empty() {
                                v
                            } else {
                                format!("{}: {v}", p.system)
                            };
                            if !p.classes.contains(&class) {
                                p.classes.push(class);
                            }
                        }
                        "FNAME" => p.fname = v,
                        "FVALUE" if !v.is_empty() => p.fvalues.push(v),
                        "FUNIT" => p.funit = v,
                        "FEATURE" => {
                            let name = std::mem::take(&mut p.fname);
                            let values = std::mem::take(&mut p.fvalues);
                            let unit = std::mem::take(&mut p.funit);
                            if !name.is_empty() && !values.is_empty() {
                                let mut value = values.join(", ");
                                if !unit.is_empty() {
                                    value = format!("{value} {unit}");
                                }
                                p.features.push((name, value));
                            }
                        }
                        "PRODUCT_FEATURES" | "ARTICLE_FEATURES" => p.system.clear(),
                        "PRODUCT" | "ARTICLE" => {
                            count += 1;
                            if let Some(done) = product.take() {
                                finish(
                                    done,
                                    count,
                                    &supplier_name,
                                    &language,
                                    &mut out,
                                    &mut deleted,
                                    &mut warnings,
                                );
                            }
                        }
                        _ => {}
                    }
                } else {
                    match (tag.as_str(), parent) {
                        ("SUPPLIER_NAME", "SUPPLIER") => supplier_name = v,
                        ("LANGUAGE", "CATALOG") if language.is_empty() => language = v.to_lowercase(),
                        _ => {}
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !seen_root {
        return Err(Error::Format("not a BMEcat catalogue (no BMECAT root element)".into()));
    }
    if !path.is_empty() {
        return Err(Error::Format("the file ends inside an element".into()));
    }
    if deleted > 0 {
        warnings.push(format!("{deleted} product(s) marked for deletion, skipped"));
    }
    Ok((out.into_values().collect(), warnings))
}

fn finish(
    p: Product,
    n: usize,
    supplier: &str,
    language: &str,
    out: &mut BTreeMap<String, Part>,
    deleted: &mut usize,
    warnings: &mut Vec<String>,
) {
    if p.mode == "delete" {
        *deleted += 1;
        return;
    }
    let order_number = if p.supplier_pid.is_empty() {
        p.manufacturer_pid.clone()
    } else {
        p.supplier_pid.clone()
    };
    if order_number.is_empty() {
        warnings.push(format!("product {n}: no order number, skipped"));
        return;
    }
    let manufacturer = if p.manufacturer.is_empty() {
        supplier.to_owned()
    } else {
        p.manufacturer.clone()
    };
    let mut part = Part {
        id: Part::make_id(&manufacturer, &order_number),
        manufacturer,
        description: pick(&p.short, language)
            .or_else(|| pick(&p.long, language))
            .unwrap_or("")
            .to_owned(),
        order_number,
        ..Default::default()
    };
    if wc_core::store::check_id(&part.id).is_err() {
        warnings.push(format!(
            "product {n}: cannot make an id from {:?}, skipped",
            part.order_number
        ));
        return;
    }
    if !p.manufacturer_pid.is_empty() && p.manufacturer_pid != part.order_number {
        part.properties.insert(PROP_MANUFACTURER_PID.into(), p.manufacturer_pid);
    }
    if !p.gtin.is_empty() {
        part.properties.insert(PROP_GTIN.into(), p.gtin);
    }
    if !p.classes.is_empty() {
        part.properties.insert(PROP_CLASS.into(), p.classes.join("; "));
    }
    for (name, value) in p.features {
        part.properties.insert(name, value);
    }
    if out.insert(part.id.clone(), part).is_some() {
        warnings.push(format!("product {n}: duplicate order number, the later product wins"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V2005: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<BMECAT version="2005" xmlns="http://www.bmecat.org/bmecat/2005">
  <HEADER>
    <CATALOG><LANGUAGE>eng</LANGUAGE><CATALOG_ID>T1</CATALOG_ID><CATALOG_VERSION>1.0</CATALOG_VERSION></CATALOG>
    <SUPPLIER><SUPPLIER_NAME>Acme Terminals</SUPPLIER_NAME></SUPPLIER>
  </HEADER>
  <T_NEW_CATALOG>
    <PRODUCT mode="new">
      <SUPPLIER_PID type="supplier_specific">3031212</SUPPLIER_PID>
      <PRODUCT_DETAILS>
        <DESCRIPTION_SHORT lang="deu">Durchgangsklemme</DESCRIPTION_SHORT>
        <DESCRIPTION_SHORT lang="eng">Feed-through terminal block</DESCRIPTION_SHORT>
        <INTERNATIONAL_PID type="gtin">4017918186722</INTERNATIONAL_PID>
        <MANUFACTURER_PID>ST 2,5</MANUFACTURER_PID>
        <MANUFACTURER_NAME>Acme Terminals</MANUFACTURER_NAME>
      </PRODUCT_DETAILS>
      <PRODUCT_FEATURES>
        <REFERENCE_FEATURE_SYSTEM_NAME>ECLASS-11.0</REFERENCE_FEATURE_SYSTEM_NAME>
        <REFERENCE_FEATURE_GROUP_ID>27141120</REFERENCE_FEATURE_GROUP_ID>
        <FEATURE><FNAME>Rated cross-section</FNAME><FVALUE>2.5</FVALUE><FUNIT>mm²</FUNIT></FEATURE>
        <FEATURE><FNAME>Colour</FNAME><FVALUE>grey</FVALUE><FVALUE>blue</FVALUE></FEATURE>
      </PRODUCT_FEATURES>
    </PRODUCT>
    <PRODUCT>
      <SUPPLIER_PID>3031225</SUPPLIER_PID>
      <PRODUCT_DETAILS><DESCRIPTION_SHORT><![CDATA[Ground terminal & PE]]></DESCRIPTION_SHORT></PRODUCT_DETAILS>
    </PRODUCT>
    <PRODUCT>
      <SUPPLIER_PID>3031238</SUPPLIER_PID>
      <PRODUCT_DETAILS>
        <DESCRIPTION_LONG>Fuse terminal block, long text only</DESCRIPTION_LONG>
        <MANUFACTURER_NAME>Other Maker</MANUFACTURER_NAME>
        <MANUFACTURER_PID>3031238</MANUFACTURER_PID>
      </PRODUCT_DETAILS>
    </PRODUCT>
    <PRODUCT><PRODUCT_DETAILS><DESCRIPTION_SHORT>no number</DESCRIPTION_SHORT></PRODUCT_DETAILS></PRODUCT>
  </T_NEW_CATALOG>
</BMECAT>"#;

    #[test]
    fn reads_a_2005_catalogue() -> Result<()> {
        let (parts, warnings) = parts(V2005)?;
        assert_eq!(parts.len(), 3, "{warnings:?}");
        assert_eq!(warnings, vec!["product 4: no order number, skipped".to_owned()]);
        let by_id: BTreeMap<&str, &Part> = parts.iter().map(|p| (p.id.as_str(), p)).collect();
        let st = by_id["acme-terminals.3031212"];
        assert_eq!(st.manufacturer, "Acme Terminals");
        assert_eq!(st.order_number, "3031212");
        assert_eq!(st.description, "Feed-through terminal block");
        let props: Vec<(&str, &str)> = st.properties.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(
            props,
            vec![
                ("Classification", "ECLASS-11.0: 27141120"),
                ("Colour", "grey, blue"),
                ("GTIN", "4017918186722"),
                ("ManufacturerPartNumber", "ST 2,5"),
                ("Rated cross-section", "2.5 mm²"),
            ]
        );
        // The supplier is the maker when the product names none; CDATA is read.
        let pe = by_id["acme-terminals.3031225"];
        assert_eq!(pe.description, "Ground terminal & PE");
        // Long text when there is no short one; the same maker number is not repeated.
        let fuse = by_id["other-maker.3031238"];
        assert_eq!(fuse.description, "Fuse terminal block, long text only");
        assert!(fuse.properties.is_empty(), "{:?}", fuse.properties);
        Ok(())
    }

    #[test]
    fn reads_a_1_2_catalogue() -> Result<()> {
        let v12 = r#"<?xml version="1.0" encoding="ISO-8859-1"?>
<BMECAT version="1.2" xmlns="http://www.bmecat.org/bmecat/1.2/bmecat_new_catalog">
  <HEADER>
    <CATALOG><LANGUAGE>deu</LANGUAGE></CATALOG>
    <SUPPLIER><SUPPLIER_NAME>Beispiel GmbH</SUPPLIER_NAME></SUPPLIER>
  </HEADER>
  <T_NEW_CATALOG>
    <ARTICLE mode="new">
      <SUPPLIER_AID>A-100</SUPPLIER_AID>
      <ARTICLE_DETAILS>
        <DESCRIPTION_SHORT>Schütz 3-polig</DESCRIPTION_SHORT>
        <EAN>4000000000017</EAN>
        <MANUFACTURER_AID>LC1D09</MANUFACTURER_AID>
        <MANUFACTURER_NAME>Maker AG</MANUFACTURER_NAME>
      </ARTICLE_DETAILS>
      <ARTICLE_FEATURES>
        <REFERENCE_FEATURE_SYSTEM_NAME>ECLASS-5.1</REFERENCE_FEATURE_SYSTEM_NAME>
        <REFERENCE_FEATURE_GROUP_ID>27-37-10-03</REFERENCE_FEATURE_GROUP_ID>
        <FEATURE><FNAME>Bemessungsspannung</FNAME><FVALUE>24</FVALUE><FUNIT>V</FUNIT></FEATURE>
      </ARTICLE_FEATURES>
    </ARTICLE>
    <ARTICLE mode="delete"><SUPPLIER_AID>A-101</SUPPLIER_AID></ARTICLE>
  </T_NEW_CATALOG>
</BMECAT>"#;
        let (parts, warnings) = parts(v12)?;
        assert_eq!(warnings, vec!["1 product(s) marked for deletion, skipped".to_owned()]);
        let [p] = parts.as_slice() else {
            return Err(Error::Format(format!("{parts:?}")));
        };
        assert_eq!(p.id, "maker-ag.a-100");
        assert_eq!(p.description, "Schütz 3-polig");
        assert_eq!(p.properties["ManufacturerPartNumber"], "LC1D09");
        assert_eq!(p.properties["GTIN"], "4000000000017");
        assert_eq!(p.properties["Classification"], "ECLASS-5.1: 27-37-10-03");
        assert_eq!(p.properties["Bemessungsspannung"], "24 V");
        Ok(())
    }

    #[test]
    fn malformed_files_are_errors_not_crashes() {
        assert!(parts("").is_err());
        assert!(parts("<catalog/>").is_err());
        assert!(parts("<BMECAT>").is_err());
        assert!(parts("<BMECAT><T_NEW_CATALOG><PRODUCT></T_NEW_CATALOG></BMECAT>").is_err());
        assert!(parts(&format!("<BMECAT>{}", "<a>".repeat(100_000))).is_err());
        assert!(parts("<BMECAT></BMECAT>").is_ok_and(|(p, w)| p.is_empty() && w.is_empty()));
        // A product without details still needs an order number.
        let r = parts(
            "<BMECAT><T_NEW_CATALOG><PRODUCT/><PRODUCT><SUPPLIER_PID>  </SUPPLIER_PID></PRODUCT></T_NEW_CATALOG></BMECAT>",
        );
        assert!(r.is_ok_and(|(p, w)| p.is_empty() && w.len() == 1), "");
    }
}
