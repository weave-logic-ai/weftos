//! Markdown to Slack mrkdwn converter.
//!
//! Slack uses its own markup format called `mrkdwn`:
//! - Bold: `*text*`
//! - Italic: `_text_`
//! - Code: `` `text` ``
//! - Code block: `` ```text``` ``
//! - Link: `<url|text>`
//!
//! See <https://api.slack.com/reference/surfaces/formatting>.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use super::MarkdownConverter;

/// Converts CommonMark to Slack mrkdwn format.
pub struct SlackMarkdownConverter;

impl MarkdownConverter for SlackMarkdownConverter {
    fn convert(&self, markdown: &str) -> String {
        let options = Options::all();
        let parser = Parser::new_ext(markdown, options);
        let mut output = String::with_capacity(markdown.len());
        let mut in_code_block = false;

        for event in parser {
            match event {
                Event::Start(tag) => match tag {
                    Tag::Strong => output.push('*'),
                    Tag::Emphasis => output.push('_'),
                    Tag::CodeBlock(_) => {
                        output.push_str("```\n");
                        in_code_block = true;
                    }
                    Tag::Link { dest_url, .. } => {
                        output.push('<');
                        output.push_str(&dest_url);
                        output.push('|');
                    }
                    Tag::Paragraph => {}
                    Tag::Heading { .. } => output.push('*'),
                    Tag::List(_) => {}
                    Tag::Item => output.push_str("- "),
                    Tag::BlockQuote(_) => output.push_str("> "),
                    Tag::Strikethrough => output.push('~'),
                    _ => {}
                },
                Event::End(tag_end) => match tag_end {
                    TagEnd::Strong => output.push('*'),
                    TagEnd::Emphasis => output.push('_'),
                    TagEnd::CodeBlock => {
                        output.push_str("```\n");
                        in_code_block = false;
                    }
                    TagEnd::Link => output.push('>'),
                    TagEnd::Paragraph => output.push('\n'),
                    TagEnd::Heading(_) => {
                        output.push('*');
                        output.push('\n');
                    }
                    TagEnd::Item => output.push('\n'),
                    TagEnd::Strikethrough => output.push('~'),
                    _ => {}
                },
                Event::Text(text) => {
                    output.push_str(&text);
                }
                Event::Code(code) => {
                    output.push('`');
                    output.push_str(&code);
                    output.push('`');
                }
                Event::SoftBreak | Event::HardBreak
                    if !in_code_block => {
                        output.push('\n');
                    }
                _ => {}
            }
        }

        output.trim_end().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(md: &str) -> String {
        SlackMarkdownConverter.convert(md)
    }

    #[test]
    fn test_bold() {
        assert_eq!(convert("**bold**"), "*bold*");
    }

    #[test]
    fn test_italic() {
        assert_eq!(convert("*italic*"), "_italic_");
    }

    #[test]
    fn test_code_inline() {
        assert_eq!(convert("`code`"), "`code`");
    }

    #[test]
    fn test_code_block() {
        let result = convert("```\nhello world\n```");
        assert!(result.contains("```"));
        assert!(result.contains("hello world"));
    }

    #[test]
    fn test_link() {
        assert_eq!(convert("[text](https://url)"), "<https://url|text>");
    }

    #[test]
    fn test_combined() {
        let result = convert("**bold** and *italic* and `code`");
        assert!(result.contains("*bold*"));
        assert!(result.contains("_italic_"));
        assert!(result.contains("`code`"));
    }

    #[test]
    fn test_heading() {
        let result = convert("# Title");
        assert!(result.contains("*Title*"));
    }

    #[test]
    fn test_strikethrough() {
        let result = convert("~~deleted~~");
        assert!(result.contains("~deleted~"));
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(convert(""), "");
    }

    #[test]
    fn test_plain_text() {
        assert_eq!(convert("hello world"), "hello world");
    }

    #[test]
    fn test_list_items() {
        let result = convert("- item one\n- item two");
        assert!(result.contains("- item one"));
        assert!(result.contains("- item two"));
    }

    #[test]
    fn test_no_html_escaping() {
        // Slack does not use HTML escaping.
        let result = convert("a & b < c > d");
        assert!(result.contains("&"));
        assert!(result.contains("<"));
        assert!(result.contains(">"));
    }

    #[test]
    fn test_blockquote() {
        let result = convert("> quoted text");
        assert!(result.contains("> "));
        assert!(result.contains("quoted text"));
    }
}
