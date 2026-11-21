//! What was agreed, and documents checked against it. When a deal is settled the AI records it (`agree`); when a
//! rate confirmation or invoice comes in citing the same reference, the core compares the numbers. A match is filed
//! with no model involved; only a difference is brought up, spelled out.

use crate::memory::Memory;
use serde_json::{json, Value};

pub fn id(reference: &str) -> String {
    let slug: String = reference.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '-' }).collect();
    format!("obj:agreement/{}", slug.to_lowercase())
}

fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < 0.5,
        _ => crate::engine::display(a).trim().eq_ignore_ascii_case(crate::engine::display(b).trim()),
    }
}

/// For each claim that cites a reference with an agreement: which agreed terms it states, and any that differ.
/// The agreement is marked confirmed or disputed and the document is linked to it.
pub fn check(mem: &Memory, claims: &[Value], document: &Value) -> Vec<Value> {
    let mut out = vec![];
    for c in claims {
        let Some(reference) = c["reference"].as_str().filter(|r| !r.is_empty()) else { continue };
        let aid = id(reference);
        let Some(a) = mem.object(&aid) else { continue };
        let terms = a["terms"].as_object().cloned().unwrap_or_default();
        let (mut compared, mut differences) = (vec![], vec![]);
        for (field, agreed) in &terms {
            // An invoice states an amount; the agreement may call the same money a rate.
            let claimed = c.get(field).filter(|v| !v.is_null()).or_else(|| match field.as_str() {
                "rate_cents" => c.get("amount_cents"),
                "amount_cents" => c.get("rate_cents"),
                _ => None,
            }).filter(|v| !v.is_null());
            let Some(claimed) = claimed else { continue };
            compared.push(field.clone());
            if !same(agreed, claimed) {
                differences.push(json!({"field": field, "agreed": agreed, "claimed": claimed}));
            }
        }
        let matches = !compared.is_empty() && differences.is_empty();
        let human = a["confirmed_by_human"] != false; // records from before this field existed were the human's
        let mut docs = a["documents"].as_array().cloned().unwrap_or_default();
        docs.push(json!({"document": document, "claim": c["id"], "matches": matches}));
        // A proposed agreement isn't confirmed by a document agreeing with it: the document may be where it came from.
        let status = if !differences.is_empty() { "disputed" } else if matches && human { "confirmed" } else { a["status"].as_str().unwrap_or("agreed") };
        mem.patch_object(&aid, &json!({"documents": docs, "status": status}));
        out.push(json!({"agreement": aid, "reference": reference, "with": a["with"], "terms": terms, "compared": compared,
                        "differences": differences, "matches": matches, "confirmed_by_human": human, "claim": c["id"]}));
    }
    out
}

/// The human confirming (or correcting) a proposed agreement.
pub fn confirm(mem: &Memory, reference: &str, terms: Option<&Value>) -> Value {
    let aid = id(reference);
    if mem.object(&aid).is_none() {
        return json!({"ok": false, "error": format!("no agreement on {reference}")});
    }
    let mut patch = json!({"confirmed_by_human": true, "status": "agreed", "confirmed": crate::memory::now()});
    if let Some(t) = terms.filter(|t| t.is_object()) {
        patch["terms"] = t.clone();
    }
    json!({"ok": mem.patch_object(&aid, &patch), "agreement": aid})
}
