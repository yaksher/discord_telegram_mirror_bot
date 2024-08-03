use crate::discord as d;
use crate::telegram as t;

pub fn discord_to_telegram_format(content: &str) -> String {
    use discord_md::ast::{MarkdownElement, MarkdownElementCollection};

    fn element_to_telegram(element: &MarkdownElement) -> String {
        match element {
            MarkdownElement::Bold(x) => format!("<b>{}</b>", collection_to_telegram(x.content())),
            MarkdownElement::ItalicsStar(x) => format!("<i>{}</i>", x.content()),
            MarkdownElement::ItalicsUnderscore(x) => format!("<i>{}</i>", x.content()),
            MarkdownElement::Strikethrough(x) => format!("<s>{}</s>", x.content()),
            MarkdownElement::Underline(x) => format!("<u>{}</u>", x.content()),
            MarkdownElement::Spoiler(x) => {
                format!("<tg-spoiler>{}</tg-spoiler>", x.content())
            }
            MarkdownElement::OneLineCode(x) => format!("<code>{}</code>", x.content()),
            MarkdownElement::MultiLineCode(x) => {
                if let Some(language) = x.language() {
                    format!(
                        "<pre><code class=\"language-{}\">{}</code></pre>",
                        language,
                        x.content()
                    )
                } else {
                    format!("<pre>{}</pre>", x.content())
                }
            }
            MarkdownElement::BlockQuote(x) => format!("<blockquote>{}</blockquote>", x.content()),
            MarkdownElement::Plain(x) => x
                .content()
                .replace("&", "&amp;")
                .replace("<", "&lt;")
                .replace(">", "&gt;"),
            // MarkdownElement::Bold(bold) => format!("*{}*", collection_to_telegram(bold.content())),
            // MarkdownElement::ItalicsStar(italic) => {
            //     format!("_{}_", collection_to_telegram(italic.content()))
            // }
            // MarkdownElement::ItalicsUnderscore(italic) => {
            //     format!("_{}_", collection_to_telegram(italic.content()))
            // }
            // MarkdownElement::Strikethrough(strikethrough) => {
            //     format!("~{}~", collection_to_telegram(strikethrough.content()))
            // }
            // // Other elements get passed directly to telegram's markdownv2 parser
            // _ => element.to_string(),
        }
    }

    fn collection_to_telegram(collection: &MarkdownElementCollection) -> String {
        collection.get().iter().map(element_to_telegram).collect()
    }

    // Parse Discord markdown to AST
    let ast = discord_md::parse(content);

    // Convert AST to Telegram HTML
    collection_to_telegram(&ast.content()).replace(".", ".")
}

pub async fn discord_author_name(ctx: &d::Context, msg: &d::Message) -> String {
    msg.author_nick(ctx.clone())
        .await
        .or_else(|| msg.author.global_name.clone())
        .unwrap_or_else(|| msg.author.name.clone())
}

pub async fn discord_reactor_name(ctx: &d::Context, reaction: &d::Reaction) -> String {
    if let Some(member) = &reaction.member {
        member
            .nick
            .clone()
            .or_else(|| member.user.global_name.clone())
            .unwrap_or_else(|| member.user.name.clone())
    } else if let Some(user_id) = reaction.user_id {
        user_id
            .to_user(ctx)
            .await
            .map(|user| user.global_name.unwrap_or(user.name))
            .unwrap_or_else(|_| "Unknown User".to_string())
    } else {
        "Unknown User".to_string()
    }
}

pub fn telegram_to_discord_format(content: &str, entities: Vec<t::MessageEntityRef>) -> String {
    use std::collections::BTreeMap;
    let mut inserts: BTreeMap<usize, String> = BTreeMap::new();
    for entity in entities {
        match entity.kind() {
            t::MessageEntityKind::Bold => {
                inserts
                    .entry(entity.start())
                    .and_modify(|s| s.push_str("**"))
                    .or_insert("**".into());
                inserts
                    .entry(entity.end())
                    .and_modify(|s| *s = format!("**{s}"))
                    .or_insert("**".into());
            }
            t::MessageEntityKind::Italic => {
                inserts
                    .entry(entity.start())
                    .and_modify(|s| s.push_str("_"))
                    .or_insert("_".into());
                inserts
                    .entry(entity.end())
                    .and_modify(|s| *s = format!("_{s}"))
                    .or_insert("_".into());
            }
            t::MessageEntityKind::Underline => {
                inserts
                    .entry(entity.start())
                    .and_modify(|s| s.push_str("__"))
                    .or_insert("__".into());
                inserts
                    .entry(entity.end())
                    .and_modify(|s| *s = format!("__{s}"))
                    .or_insert("__".into());
            }
            t::MessageEntityKind::Strikethrough => {
                inserts
                    .entry(entity.start())
                    .and_modify(|s| s.push_str("~~"))
                    .or_insert("~~".into());
                inserts
                    .entry(entity.end())
                    .and_modify(|s| *s = format!("~~{s}"))
                    .or_insert("~~".into());
            }
            t::MessageEntityKind::Spoiler => {
                inserts
                    .entry(entity.start())
                    .and_modify(|s| s.push_str("||"))
                    .or_insert("||".into());
                inserts
                    .entry(entity.end())
                    .and_modify(|s| *s = format!("||{s}"))
                    .or_insert("||".into());
            }
            t::MessageEntityKind::Code => {
                inserts
                    .entry(entity.start())
                    .and_modify(|s| s.push_str("`"))
                    .or_insert("`".into());
                inserts
                    .entry(entity.end())
                    .and_modify(|s| *s = format!("`{s}"))
                    .or_insert("`".into());
            }
            t::MessageEntityKind::Pre { language } => {
                inserts.insert(
                    entity.start(),
                    format!("```{}", language.as_deref().unwrap_or("")),
                );
                inserts.insert(entity.end(), "```".to_string());
            }
            t::MessageEntityKind::TextLink { url } => {
                inserts
                    .entry(entity.start())
                    .and_modify(|s| s.push_str("["))
                    .or_insert("[".into());
                inserts
                    .entry(entity.end())
                    .and_modify(|s| *s = format!("{s}]({url})"))
                    .or_insert(format!("]({url})",));
            }
            t::MessageEntityKind::TextMention { user } => {
                // Handle TextMention
                let _ = user;
            }
            t::MessageEntityKind::CustomEmoji { custom_emoji_id } => {
                let _ = custom_emoji_id;
            }
            t::MessageEntityKind::Mention
            | t::MessageEntityKind::Hashtag
            | t::MessageEntityKind::Cashtag
            | t::MessageEntityKind::BotCommand
            | t::MessageEntityKind::Url
            | t::MessageEntityKind::Email
            | t::MessageEntityKind::PhoneNumber => {
                // Discord either doesn't support or automatically formats these
                // so no work needs to be done
            }
        }
    }
    let mut positions = vec![0];
    positions.extend(inserts.keys().copied());
    (&positions)
        .iter()
        .zip(&positions[1..])
        .flat_map(|(&i, &j)| [&content[i..j], &inserts[&j]])
        .chain(Some(&content[*positions.last().unwrap()..]))
        .collect()
}

pub fn telegram_author_name(msg: &t::Message) -> String {
    msg.from()
        .map(|u| u.full_name())
        .or(msg.sender_chat().and_then(|c| c.title().map(Into::into)))
        .unwrap_or("Unknown".into())
}

pub fn telegram_forwarded_from_name(f: &t::ForwardedFrom) -> String {
    match f {
        t::ForwardedFrom::User(user) => user.full_name(),
        t::ForwardedFrom::Chat(chat) => chat
            .title()
            .or_else(|| chat.username())
            .unwrap_or("Unknown")
            .to_string(),
        t::ForwardedFrom::SenderName(name) => name.to_string(),
    }
}

pub fn parse_telegram_reaction_message(
    text: &str,
) -> std::collections::HashMap<String, Vec<String>> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(2, ':').collect();
            if parts.len() == 2 {
                Some((
                    parts[0]
                        .trim_matches(|c: char| {
                            c.is_whitespace() || c == '<' || c == '>' || c == 'b' || c == '/'
                        })
                        .to_string(),
                    parts[1].split(',').map(|s| s.trim().to_string()).collect(),
                ))
            } else {
                None
            }
        })
        .collect()
}

pub fn format_telegram_reaction_message(
    reactions: &std::collections::HashMap<String, Vec<String>>,
) -> String {
    Some("<b>Reactions</b>".to_string())
        .into_iter()
        .chain(
            reactions
                .iter()
                .filter(|(_, emojis)| !emojis.is_empty())
                .map(|(user, emojis)| format!("<b>{}</b>: {}", user, emojis.join(", "))),
        )
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn parse_discord_reaction_message(text: &str) -> std::collections::HashMap<String, String> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(2, ':').collect();
            if parts.len() == 2 {
                Some((
                    parts[0]
                        .trim_matches(|c: char| c.is_whitespace() || c == '*' || c == '_')
                        .to_string(),
                    parts[1].trim().to_string(),
                ))
            } else {
                None
            }
        })
        .collect()
}

pub fn format_discord_reaction_message(
    reactions: &std::collections::HashMap<String, String>,
) -> String {
    Some("**Reactions**".to_string())
        .into_iter()
        .chain(
            reactions
                .iter()
                .map(|(user, emoji)| format!("**{}**: {}", user, emoji)),
        )
        .collect::<Vec<_>>()
        .join("\n")
}
