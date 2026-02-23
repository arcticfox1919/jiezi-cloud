//! `[[WikiLink]]` parser.
//!
//! Scans a Markdown document for links in the `[[Title]]` or `[[Title|Alias]]`
//! syntax commonly used in note-taking tools (Obsidian, Roam, etc.) and
//! returns the list of referenced note titles.
//!
//! # Format
//!
//! | Syntax                | Title          | Display text    |
//! |-----------------------|----------------|-----------------|
//! | `[[Rust]]`            | "Rust"         | "Rust"          |
//! | `[[Rust|the language]]` | "Rust"       | "the language"  |
//!
//! The parser uses a simple byte-scan rather than a full Markdown parser
//! because WikiLinks are **not** standard CommonMark and would be treated as
//! plain text by `pulldown-cmark`.  The standard Markdown pass is still run
//! separately for FTS content extraction.

/// A single WikiLink found in a Markdown document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiLink {
    /// Target note title as written inside `[[…]]`.
    pub target: String,
    /// Display alias when the syntax is `[[Target|Alias]]`.
    /// Falls back to `target` when no alias is present.
    pub display: String,
}

/// Extract all `[[WikiLink]]` references from `markdown`.
///
/// Performs a single linear scan — O(n) in document length.
pub fn extract(markdown: &str) -> Vec<WikiLink> {
    let mut links = Vec::new();
    let bytes = markdown.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i + 1 < len {
        // Look for '[['
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            i += 2;
            let start = i;
            // Scan until ']]' or end of input
            while i + 1 < len && !(bytes[i] == b']' && bytes[i + 1] == b']') {
                i += 1;
            }
            if i + 1 < len {
                let inner = &markdown[start..i];
                let link = parse_inner(inner);
                links.push(link);
                i += 2; // skip ']]'
            }
        } else {
            i += 1;
        }
    }

    links
}

/// Parse the text inside `[[…]]` into a [`WikiLink`].
fn parse_inner(inner: &str) -> WikiLink {
    if let Some(pipe) = inner.find('|') {
        WikiLink {
            target: inner[..pipe].trim().to_owned(),
            display: inner[pipe + 1..].trim().to_owned(),
        }
    } else {
        let title = inner.trim().to_owned();
        WikiLink {
            target: title.clone(),
            display: title,
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_link() {
        let links = extract("See [[Rust]] for details.");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Rust");
        assert_eq!(links[0].display, "Rust");
    }

    #[test]
    fn test_alias_link() {
        let links = extract("See [[Rust|the language]] for details.");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Rust");
        assert_eq!(links[0].display, "the language");
    }

    #[test]
    fn test_multiple_links() {
        let links = extract("[[冯诺依曼架构]] relates to [[图灵机]].");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "冯诺依曼架构");
        assert_eq!(links[1].target, "图灵机");
    }

    #[test]
    fn test_no_links() {
        let links = extract("No links here.");
        assert!(links.is_empty());
    }

    #[test]
    fn test_unterminated_link_ignored() {
        let links = extract("This [[never closes");
        assert!(links.is_empty());
    }
}
