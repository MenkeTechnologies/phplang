//! Offline generator for `docs/reference.html` — the keyword / construct / cast /
//! library-function reference page, rendered with the same cyberpunk HUD chrome
//! as `docs/index.html`. Run before publishing GitHub Pages:
//!
//! ```sh
//! cargo run --bin gen-docs
//! ```
//!
//! Source of truth: the corpus in `phplang::lsp` (`corpus()`), the exact
//! `(name, chapter, doc, example)` table used for documentation. The static page
//! therefore never drifts from the runtime — a name is documented here only if
//! phplang actually recognizes it in `lexer.rs`/`parser.rs` (keywords, constructs,
//! casts) or `builtins.rs` (library functions).
//!
//! Chapters are the corpus's own grouping (`entry.1`), rendered in first-seen
//! order, one `<section>` per chapter, one `<article class="doc-entry">` per name
//! with a runnable usage example.

use std::collections::BTreeSet;
use std::fmt::Write as _;

fn main() {
    let corpus = phplang::lsp::corpus();
    let chapters: BTreeSet<&str> = corpus.iter().map(|(_, c, _, _)| *c).collect();

    let page = format!(
        "{head}{body}{foot}",
        head = HEAD,
        body = build_body(corpus),
        foot = FOOT,
    )
    // Stamp the current crate version so the page never falls behind Cargo.toml
    // (the meta version-sync gate compares docs/*.html against the manifest).
    .replace("__PHPLANG_VERSION__", env!("CARGO_PKG_VERSION"));

    let out = "docs/reference.html";
    if let Err(e) = std::fs::write(out, page) {
        eprintln!("gen-docs: cannot write {out}: {e}");
        std::process::exit(1);
    }
    println!(
        "wrote {out} ({} entries, {} chapters)",
        corpus.len(),
        chapters.len()
    );
}

/// Render one `<section>` per chapter, each holding one `<article class="doc-entry">`
/// per name: name heading, one-line description, and a runnable usage example.
///
/// Chapters are grouped in first-seen order and every entry of a chapter lands
/// in that chapter's single section — even when the corpus interleaves chapters.
/// Grouping keeps each `id="ch-…"` anchor unique, so the reference PDF never
/// emits a multiply-defined label.
fn build_body(corpus: &[(&str, &str, &str, &str)]) -> String {
    // Ordered list of chapters (first-seen) plus each chapter's entries. A plain
    // Vec of (chapter, Vec<entry>) keeps insertion order without a dependency.
    let mut chapters: Vec<(&str, Vec<&(&str, &str, &str, &str)>)> = Vec::new();
    for entry in corpus {
        let chapter = entry.1;
        match chapters.iter_mut().find(|(c, _)| *c == chapter) {
            Some((_, entries)) => entries.push(entry),
            None => chapters.push((chapter, vec![entry])),
        }
    }

    let mut out = String::new();
    for (chapter, entries) in &chapters {
        // The `id="ch-…"` marks this as a real reference chapter. The
        // reference-PDF pipeline keeps id-carrying sections and drops the
        // id-less ones (page chrome / link lists), so every chapter needs it.
        let _ = write!(
            out,
            "\n      <section class=\"tutorial-section\" id=\"ch-{slug}\">\n\
             \x20       <h2>{title}</h2>\n",
            slug = slugify(chapter),
            title = html_escape(chapter),
        );
        // A per-chapter counter keeps the `doc-…` anchor ids unique even when a
        // name repeats across chapters.
        for (idx, (name, _chapter, doc, example)) in entries.iter().enumerate() {
            let anchor = format!("doc-{}-{}", slugify(chapter), idx + 1);
            let _ = write!(
                out,
                "        <article class=\"doc-entry\" id=\"{anchor}\">\n\
                 \x20         <h3><a class=\"doc-anchor\" href=\"#{anchor}\">#</a> <code>{name}</code></h3>\n\
                 \x20         <p>{doc}</p>\n\
                 \x20         <pre><code class=\"lang-php\">{example}</code></pre>\n\
                 \x20       </article>\n",
                anchor = anchor,
                name = html_escape(name),
                doc = html_escape(doc),
                example = html_escape(example),
            );
        }
        out.push_str("      </section>\n");
    }
    out
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Lowercase, non-alphanumeric runs collapsed to a single `-`, edges trimmed —
/// e.g. `Keyword` -> `keyword`, `String` -> `string`. Used for the `id="ch-…"`
/// chapter anchors.
fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

const HEAD: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="color-scheme" content="dark light">
  <meta name="description" content="phplang — Language reference. Keywords, language constructs, casts, and library functions available in the current phplang build. MIT licensed.">
  <title>phplang &mdash; Language Reference</title>
  <link rel="preconnect" href="https://fonts.googleapis.com">
  <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
  <link href="https://fonts.googleapis.com/css2?family=Orbitron:wght@400;600;700;900&family=Share+Tech+Mono&display=swap" rel="stylesheet">
  <link rel="stylesheet" href="hud-static.css">
  <link rel="stylesheet" href="tutorial.css">
  <style>
    .tutorial-main { max-width: 76rem; }
    .file-table { width:100%;border-collapse:collapse;margin:0.6rem 0;font-size:12px; }
    .file-table th { background:var(--bg-secondary);color:var(--cyan);font-family:'Orbitron',sans-serif;font-size:10px;font-weight:700;letter-spacing:1.2px;text-transform:uppercase;text-align:left;padding:7px 10px;border:1px solid var(--border); }
    .file-table td { padding:6px 10px;border:1px solid var(--border);color:var(--text-dim);vertical-align:middle; }
    .file-table tr:hover td { background:var(--bg-hover); }
    .file-table td:first-child { font-family:'Share Tech Mono',monospace;color:var(--accent-light);font-weight:600;white-space:nowrap; }
    .file-table code { font-size:11px;color:var(--accent-light);background:var(--bg-primary);padding:1px 4px;border-radius:2px; }
    .section-rule { border:none;border-top:1px dashed var(--border);margin:2rem 0; }
    .hub-scheme-strip { border-bottom:1px dashed var(--border);background:color-mix(in srgb, var(--bg-secondary) 85%, transparent);padding:0.55rem 1.5rem 0.65rem;position:relative; }
    .hub-scheme-strip-inner { max-width:76rem;margin:0 auto;display:flex;align-items:center;gap:0.85rem; }
    .hub-scheme-strip .hud-scheme-label { flex:0 0 auto;font-family:'Orbitron',sans-serif;font-size:9px;font-weight:700;letter-spacing:2px;text-transform:uppercase;color:var(--accent);text-align:left; }
    .hub-scheme-strip .scheme-grid { flex:1 1 auto;display:grid;grid-template-columns:repeat(5,minmax(0,1fr));gap:6px; }
    @media (max-width:720px){ .hub-scheme-strip-inner{flex-direction:column;align-items:stretch}.hub-scheme-strip .scheme-grid{grid-template-columns:repeat(2,minmax(0,1fr))} }
    .docs-build-line { margin:0.35rem 0 0;font-family:'Share Tech Mono',ui-monospace,monospace;font-size:11px;color:var(--text-dim);letter-spacing:0.03em;max-width:52rem;opacity:0.75; }
  </style>
</head>
<body>
  <div class="app tutorial-app" id="docsApp">
    <div class="crt-scanline" id="crtH" aria-hidden="true"></div>
    <div class="crt-scanline-v" id="crtV" aria-hidden="true"></div>

    <header class="tutorial-header">
      <div class="tutorial-header-inner">
        <div>
          <h1 class="tutorial-brand">// PHPLANG — LANGUAGE REFERENCE</h1>
          <nav class="tutorial-crumbs" aria-label="Breadcrumb">
            <a href="index.html">Docs</a>
            <span class="sep">/</span>
            <span class="current">Language Reference</span>
            <span class="sep">/</span>
            <a href="https://github.com/MenkeTechnologies/phplang" target="_blank" rel="noopener noreferrer">GitHub</a>
          </nav>
          <p class="docs-build-line">phplang v__PHPLANG_VERSION__ · PHP on fusevm · lex/parse → AST → fusevm bytecode → Cranelift JIT · no bespoke VM · the first compiled standalone PHP runtime · MIT · in active development</p>
        </div>
        <div class="tutorial-toolbar">
          <button type="button" class="btn btn-secondary" id="btnTheme" title="Toggle light/dark">Theme</button>
          <button type="button" class="btn btn-secondary active" id="btnCrt" title="CRT scanline overlay">CRT</button>
          <button type="button" class="btn btn-secondary active" id="btnNeon" title="Neon border pulse">Neon</button>
          <a class="btn btn-secondary" href="index.html">Docs</a>
          <a class="btn btn-secondary" href="https://github.com/MenkeTechnologies/phplang" target="_blank" rel="noopener noreferrer">GitHub</a>
        </div>
      </div>
    </header>

    <div class="hub-scheme-strip">
      <div class="hub-scheme-strip-inner">
        <span class="hud-scheme-label">// Color scheme</span>
        <div class="scheme-grid" id="hudSchemeGrid"></div>
      </div>
    </div>

    <main class="tutorial-main">
      <h2 class="tutorial-title"><span class="step-hash">&gt;_</span>LANGUAGE REFERENCE</h2>
      <p class="tutorial-subtitle">Every reserved keyword, language construct, type cast, and library function the current phplang build recognizes, grouped by keyword then construct then cast then library area. This page is generated from the language corpus (<code>src/lsp.rs</code>) by the <code>gen-docs</code> binary, so it stays in sync with what the runtime actually implements. Keywords/constructs mirror <code>lexer.rs</code> and <code>parser.rs</code>; each library function mirrors a real dispatch arm in <code>src/builtins.rs</code>.</p>
"#;

const FOOT: &str = r#"
      <section class="tutorial-section">
        <h2>More</h2>
        <ul>
          <li><strong>Docs</strong> — <a href="index.html">index.html</a> (overview, architecture, examples)</li>
          <li><strong>Engineering report</strong> — <a href="report.html">report.html</a> (value model, status, dependencies)</li>
          <li><strong>Source</strong> — <a href="https://github.com/MenkeTechnologies/phplang">github.com/MenkeTechnologies/phplang</a></li>
        </ul>
      </section>
    </main>

  </div>

  <script src="hud-theme.js"></script>
</body>
</html>
"#;
