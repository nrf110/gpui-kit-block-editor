use super::*;

#[test]
fn malformed_ancestor_chains_reject_in_every_order_without_panicking() {
    let mut document = Document::new();
    let root = document.insert_section(None, "root").unwrap();
    let middle = document
        .insert_block(
            Some(&root),
            None,
            &BlockSpec {
                kind: SectionKind::Text,
                content: RichDocument::paragraph("middle"),
                children: vec![],
            },
        )
        .unwrap();
    document
        .insert_block(
            Some(&middle),
            None,
            &BlockSpec {
                kind: SectionKind::Text,
                content: RichDocument::paragraph("leaf"),
                children: vec![],
            },
        )
        .unwrap();
    let original = document.sections();
    let json: serde_json::Value = serde_json::from_str(&document.export_json().unwrap()).unwrap();
    for order in [[0, 1, 2], [2, 1, 0], [1, 2, 0]] {
        let mut valid = json.clone();
        valid["sections"] = order
            .iter()
            .map(|index| json["sections"][*index].clone())
            .collect();
        assert_eq!(
            Document::import_json(&valid.to_string())
                .unwrap()
                .sections(),
            original
        );
        let mut missing = valid.clone();
        for section in missing["sections"].as_array_mut().unwrap() {
            if section["id"] == root {
                section["parent"] = "missing".into();
            }
        }
        assert!(Document::import_json(&missing.to_string()).is_err());
        let mut cycle = valid;
        let leaf = original[2].id.clone();
        for section in cycle["sections"].as_array_mut().unwrap() {
            if section["id"] == root {
                section["parent"] = leaf.clone().into();
            }
        }
        assert!(Document::import_json(&cycle.to_string()).is_err());
    }
}
