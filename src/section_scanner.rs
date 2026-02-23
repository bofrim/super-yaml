//! Scanner for top-level document marker and section fences.

use regex::Regex;

use crate::error::SyamlError;

#[derive(Debug, Clone)]
/// A named document section extracted from the source text.
pub struct Section {
    /// Section name (for example `schema` or `data`).
    pub name: String,
    /// Raw section body between this fence and the next.
    pub body: String,
    /// 1-based starting line number of section body.
    pub start_line: usize,
    /// 1-based ending line number (exclusive).
    pub end_line: usize,
}

const MARKER: &str = "---!syaml/v0";

/// Scans a `.syaml` source document into `(version, ordered sections)`.
///
/// Validates marker presence, allowed section names, and uniqueness.
pub fn scan_sections(input: &str) -> Result<(String, Vec<Section>), SyamlError> {
    let lines: Vec<&str> = input.lines().collect();
    let mut first_non_empty = None;
    for (idx, line) in lines.iter().enumerate() {
        if !line.trim().is_empty() {
            first_non_empty = Some((idx, *line));
            break;
        }
    }

    let (marker_line_idx, marker_line) = first_non_empty.ok_or_else(|| {
        SyamlError::MarkerError("document is empty; expected ---!syaml/v0".to_string())
    })?;

    if marker_line.trim() != MARKER {
        return Err(SyamlError::MarkerError(format!(
            "expected first non-empty line to be '{MARKER}', found '{}'",
            marker_line.trim()
        )));
    }

    let fence_re = Regex::new(r"^---([a-z_]+)\s*$").expect("valid regex");
    let mut sections: Vec<Section> = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_start = 0usize;
    let mut current_body: Vec<&str> = Vec::new();

    for (i, line) in lines.iter().enumerate().skip(marker_line_idx + 1) {
        if let Some(cap) = fence_re.captures(line.trim()) {
            if let Some(name) = current_name.take() {
                sections.push(Section {
                    name,
                    body: current_body.join("\n"),
                    start_line: current_start,
                    end_line: i,
                });
                current_body.clear();
            }

            current_name = Some(cap[1].to_string());
            current_start = i + 1;
            continue;
        }

        if current_name.is_some() {
            current_body.push(line);
        } else if !line.trim().is_empty() {
            return Err(SyamlError::SectionError(format!(
                "content before first section fence at line {}",
                i + 1
            )));
        }
    }

    if let Some(name) = current_name.take() {
        sections.push(Section {
            name,
            body: current_body.join("\n"),
            start_line: current_start,
            end_line: lines.len(),
        });
    }

    validate_sections(&sections)?;
    Ok(("v0".to_string(), sections))
}

fn validate_sections(sections: &[Section]) -> Result<(), SyamlError> {
    let mut seen = std::collections::HashSet::new();
    for section in sections {
        if !matches!(section.name.as_str(), "meta" | "schema" | "data" | "functional" | "module") {
            return Err(SyamlError::SectionError(format!(
                "unknown section '{}'",
                section.name
            )));
        }

        if !seen.insert(section.name.clone()) {
            return Err(SyamlError::SectionError(format!(
                "duplicate section '{}'",
                section.name
            )));
        }
    }

    Ok(())
}
