use discord_md::generate::{ToMarkdownString, ToMarkdownStringOption};
use discord_md::parse;

#[test]
fn test_parse_then_generate_1() {
    let message = include_str!("example.md");
    assert_eq!(
        parse(message).to_markdown_string(&ToMarkdownStringOption::new()),
        message
    );
}

#[test]
fn test_parse_then_generate_2() {
    let message = include_str!("../README.md");
    assert_eq!(
        parse(message).to_markdown_string(&ToMarkdownStringOption::new()),
        message
    );
}
