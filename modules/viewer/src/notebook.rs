//! Jupyter notebooks (`.ipynb`): markdown cells rendered, code cells highlighted, text outputs and errors shown.
//! Images in outputs are not drawn (a note says so). The file is JSON from anywhere, so cell, output and text sizes are bounded.

use serde_json::Value;

use crate::ansi::{paint, Style, DIM};
use crate::{code, markdown, Options};

const MAX_CELLS: usize = 2_000;
/// Longest text of one output shown, in characters.
const MAX_OUTPUT: usize = 20_000;
const ERR: Style = Style::fg(9);
const PROMPT: Style = Style::fg(4);

/// A cell's `source` is one string or a list of lines.
fn source(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

fn clip(s: &str) -> String {
    if s.chars().count() > MAX_OUTPUT {
        let mut t: String = s.chars().take(MAX_OUTPUT).collect();
        t.push_str("\n\u{2026} (output cut)");
        t
    } else {
        s.to_owned()
    }
}

/// Render a notebook's bytes.
pub fn render(bytes: &[u8], opts: &Options) -> String {
    let nb: Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => {
            return format!(
                "{}\n",
                paint(&format!("cannot read the notebook: {e}"), DIM, opts.color)
            )
        }
    };
    let Some(cells) = nb.get("cells").and_then(Value::as_array) else {
        return format!(
            "{}\n",
            paint("cannot read the notebook: no cells", DIM, opts.color)
        );
    };
    let lang = nb
        .pointer("/metadata/language_info/name")
        .or_else(|| nb.pointer("/metadata/kernelspec/language"))
        .and_then(Value::as_str)
        .unwrap_or("python")
        .to_ascii_lowercase();
    let mut out = String::new();
    for (i, cell) in cells.iter().take(MAX_CELLS).enumerate() {
        let src = source(cell.get("source").unwrap_or(&Value::Null));
        match cell
            .get("cell_type")
            .and_then(Value::as_str)
            .unwrap_or("raw")
        {
            "markdown" => out.push_str(&markdown::render(&src, opts)),
            "code" => {
                let n = cell
                    .get("execution_count")
                    .and_then(Value::as_u64)
                    .map_or_else(|| " ".to_owned(), |n| n.to_string());
                out.push_str(&paint(&format!("In [{n}]:\n"), PROMPT, opts.color));
                out.push_str(&code::render(
                    &src,
                    &lang,
                    &Options {
                        line_numbers: false,
                        ..*opts
                    },
                ));
                if let Some(outputs) = cell.get("outputs").and_then(Value::as_array) {
                    for o in outputs {
                        out.push_str(&output(o, opts));
                    }
                }
            }
            _ => {
                out.push_str(&src);
                out.push('\n');
            }
        }
        if i + 1 < cells.len() {
            out.push('\n');
        }
    }
    if cells.len() > MAX_CELLS {
        out.push_str(&paint(
            &format!("(\u{2026} {} more cells)\n", cells.len() - MAX_CELLS),
            DIM,
            opts.color,
        ));
    }
    if out.trim().is_empty() {
        out = paint("(empty notebook)\n", DIM, opts.color);
    }
    out
}

fn output(o: &Value, opts: &Options) -> String {
    let mut s = String::new();
    match o.get("output_type").and_then(Value::as_str).unwrap_or("") {
        "stream" => s.push_str(&paint(
            &clip(&source(o.get("text").unwrap_or(&Value::Null))),
            DIM,
            opts.color,
        )),
        "execute_result" | "display_data" => {
            let data = o.get("data");
            if let Some(t) = data.and_then(|d| d.get("text/plain")) {
                s.push_str(&paint(&clip(&source(t)), DIM, opts.color));
            }
            if let Some(d) = data.and_then(Value::as_object) {
                for k in d.keys().filter(|k| k.starts_with("image/")) {
                    s.push_str(&paint(
                        &format!("\n[{k} output not shown here]"),
                        DIM,
                        opts.color,
                    ));
                }
            }
        }
        "error" => {
            let name = o.get("ename").and_then(Value::as_str).unwrap_or("Error");
            let value = o.get("evalue").and_then(Value::as_str).unwrap_or("");
            s.push_str(&paint(&clip(&format!("{name}: {value}")), ERR, opts.color));
        }
        _ => {}
    }
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Options {
        Options {
            width: 70,
            color: false,
            line_numbers: false,
        }
    }

    const NB: &str = r##"{
      "metadata": {"language_info": {"name": "python"}},
      "cells": [
        {"cell_type": "markdown", "source": ["# Analysis\n", "Some **notes**."]},
        {"cell_type": "code", "execution_count": 3, "source": "x = 1 + 1\nprint(x)",
         "outputs": [
           {"output_type": "stream", "name": "stdout", "text": ["2\n"]},
           {"output_type": "execute_result", "data": {"text/plain": "42", "image/png": "AAAA"}},
           {"output_type": "error", "ename": "ValueError", "evalue": "bad", "traceback": []}
         ]},
        {"cell_type": "raw", "source": "raw text"}
      ]}"##;

    #[test]
    fn cells_outputs_and_errors_are_shown() {
        let t = render(NB.as_bytes(), &plain());
        assert!(t.contains("Analysis") && t.contains("notes"), "{t}");
        assert!(
            t.contains("In [3]:") && t.contains("x = 1 + 1") && t.contains("print(x)"),
            "{t}"
        );
        assert!(t.contains("\n2\n"), "stream output: {t}");
        assert!(
            t.contains("42") && t.contains("[image/png output not shown here]"),
            "{t}"
        );
        assert!(t.contains("ValueError: bad"), "{t}");
        assert!(t.contains("raw text"));
    }

    #[test]
    fn broken_notebooks_and_huge_outputs_are_handled() {
        assert!(render(b"not json", &plain()).starts_with("cannot read the notebook"));
        assert!(render(b"{}", &plain()).contains("no cells"));
        assert!(render(br#"{"cells": []}"#, &plain()).contains("(empty notebook)"));
        let big = format!(
            r#"{{"cells":[{{"cell_type":"code","source":"x","outputs":[{{"output_type":"stream","text":"{}"}}]}}]}}"#,
            "y".repeat(50_000)
        );
        let t = render(big.as_bytes(), &plain());
        assert!(t.contains("(output cut)") && t.len() < 30_000);
        // Colour keeps the error visible.
        assert!(render(
            NB.as_bytes(),
            &Options {
                color: true,
                ..plain()
            }
        )
        .contains("\x1b[91m"));
    }
}
