#![allow(dead_code)]
mod db;
mod format;

use sqlx::SqlitePool;

use std::{env, future::IntoFuture, sync::Arc, time::Duration};

// use tokio::sync::RwLock;
#[allow(unused_imports)]
mod telegram {
    pub use teloxide::errors::{DownloadError, RequestError};
    pub use teloxide::prelude::*;
    pub use teloxide::types::*;
}

use teloxide::{dptree::endpoint, net::Download, prelude::*};
use url::Url;

use dotenv;

#[allow(unused_imports)]
mod discord {
    pub use serenity::{
        all::{content_safe, ContentSafeOptions},
        async_trait,
        builder::{CreateAttachment, CreateMessage, EditMessage},
        http::Http,
        model::{
            channel::{Message, Reaction, ReactionType},
            event::MessageUpdateEvent,
            gateway::Ready,
            id::{ChannelId, GuildId, MessageId},
        },
        prelude::*,
        Error,
    };
}
use serenity::prelude::*;

use discord as d;
use telegram as t;

const DISCORD_TOKEN_ENV: &str = "DISCORD_TOKEN";

struct DiscordState {
    telegram_bot: t::Bot,
    db: SqlitePool,
}

macro_rules! telegram_request {
    ($e:expr$(,)?) => {
        telegram_request(|| $e, || log::error!("{}:{}", file!(), line!()))
    };
    ($e:expr, $log:expr$(,)?) => {
        telegram_request(|| $e, || $log)
    };
}
macro_rules! discord_request {
    ($e:expr$(,)?) => {
        discord_request(|| $e, || log::error!("{}:{}", file!(), line!()))
    };
    ($e:expr, $log:expr$(,)?) => {
        discord_request(|| $e, || $log)
    };
}

#[d::async_trait]
impl d::EventHandler for DiscordState {
    // Set a handler for the `message` event - so that whenever a new message
    // is received - the closure (or function) passed will be called.
    //
    // Event handlers are dispatched through a threadpool, and so multiple
    // events can be dispatched simultaneously.
    async fn message(&self, ctx: d::Context, msg: d::Message) {
        if msg.author.id == ctx.cache.current_user().id {
            return;
        }
        let telegram_chat = match db::get_telegram_chat_id(msg.channel_id) {
            Some(chat_id) => chat_id,
            None => {
                log::info!("Got message {msg:?} in unregistered discord channel");
                return;
            }
        };
        let content = d::content_safe(&ctx, &msg.content, &d::ContentSafeOptions::default(), &[]);
        let content = format::discord_to_telegram_format(&content);
        let author = format::discord_author_name(&ctx, &msg).await;

        let mut text = format!("<b>{author}</b>\n{content}");

        let mut builder = self
            .telegram_bot
            .send_message(telegram_chat, "")
            .parse_mode(t::ParseMode::Html);

        if let Some(ref_msg) = msg.referenced_message.clone() {
            // if a message is being replied to, find the original message in
            // the database and reply to that message's reflection
            let found_mirror = match db::get_telegram_message_id(&self.db, ref_msg.id.into())
                .await
                .as_deref()
            {
                Ok(&[mirror_id, ..]) => {
                    builder = builder.reply_to_message_id(mirror_id);
                    true
                }
                Ok([]) => false,
                Err(e) => {
                    log::error!("Database lookup failed: {e}");
                    false
                }
            };
            // if we couldn't find the message in the database, copy the message
            // as a block quote
            if !found_mirror {
                let ref_content = format::discord_to_telegram_format(&ref_msg.content);
                let ref_author = format::discord_author_name(&ctx, &ref_msg).await;
                text = format!(
                    "<blockquote expandable><b>{ref_author}</b>\n{ref_content}</blockquote>\n{text}"
                );
            }
        }

        let builder = builder.text(text);

        let telegram_result = telegram_request!(
            builder.clone(),
            log::error!(
                "{}:{} Sender: {}\nText: {}\nFormatted: {}",
                file!(),
                line!(),
                author,
                msg.content,
                content
            ),
        )
        .await;

        if let Some(telegram_msg) = telegram_result {
            if let Err(e) =
                db::insert_mapping(&self.db, msg.id, telegram_msg.id, telegram_chat).await
            {
                log::error!("Failed to insert message mapping: {}", e);
            }
        }

        if msg.attachments.iter().all(|a| {
            a.content_type
                .as_deref()
                .is_some_and(|t| t.starts_with("image/"))
        }) && msg.attachments.len() > 1
        {
            let telegram_result = telegram_request!(self.telegram_bot.send_media_group(
                telegram_chat,
                msg.attachments
                    .iter()
                    .map(|a| a.url.as_str())
                    .map(url::Url::parse)
                    .filter_map(|url| {
                        url.inspect_err(|e| log::error!("Failed to parse attachment url: {e}"))
                            .ok()
                    })
                    .map(t::InputFile::url)
                    .zip(msg.attachments.iter())
                    .map(|(m, a)| (
                        m.file_name(a.filename.trim_start_matches("SPOILER_").to_string()),
                        a.filename.starts_with("SPOILER_")
                    ))
                    .map(|(m, s)| {
                        let m = t::InputMediaPhoto::new(m);
                        if s {
                            m.spoiler()
                        } else {
                            m
                        }
                    })
                    .map(t::InputMedia::Photo)
                    .collect::<Vec<_>>(),
            ))
            .await;
            if let Some(telegram_result) = telegram_result {
                for telegram_msg in telegram_result {
                    if let Err(e) =
                        db::insert_mapping(&self.db, msg.id, telegram_msg.id, telegram_chat).await
                    {
                        log::error!("Failed to insert message mapping: {}", e);
                    }
                }
            }
        } else {
            let futs = msg
                .attachments
                .into_iter()
                .map(|a| async move {
                    let url = match Url::parse(a.url.as_str()) {
                        Ok(url) => url,
                        Err(e) => {
                            log::error!("Failed to parse attachment url: {e}");
                            return;
                        }
                    };
                    let mut telegram_msg = None;
                    let matched = if let Some(kind) = a.content_type.as_deref() {
                        if kind.starts_with("image/") {
                            let f = t::InputFile::url(url.clone())
                                .file_name(a.filename.trim_start_matches("SPOILER_").to_string());
                            let s = self
                                .telegram_bot
                                .send_photo(telegram_chat, f)
                                .has_spoiler(a.filename.starts_with("SPOILER_"));
                            telegram_msg = telegram_request!(s.clone()).await;
                            true
                        } else if kind.starts_with("video/") {
                            let f = t::InputFile::url(url.clone())
                                .file_name(a.filename.trim_start_matches("SPOILER_").to_string());
                            let s = self
                                .telegram_bot
                                .send_video(telegram_chat, f)
                                .has_spoiler(a.filename.starts_with("SPOILER_"));
                            telegram_msg = telegram_request!(s.clone()).await;
                            true
                        } else if kind.starts_with("audio/") {
                            telegram_msg = telegram_request!(self
                                .telegram_bot
                                .send_audio(telegram_chat, t::InputFile::url(url.clone())))
                            .await;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if !matched {
                        telegram_msg = telegram_request!(self
                            .telegram_bot
                            .send_document(telegram_chat, t::InputFile::url(url.clone())))
                        .await;
                    }
                    if let Some(telegram_msg) = telegram_msg {
                        if let Err(e) =
                            db::insert_mapping(&self.db, msg.id, telegram_msg.id, telegram_chat)
                                .await
                        {
                            log::error!("Failed to insert message mapping: {}", e);
                        }
                    }
                })
                .collect::<Vec<_>>();
            futures::future::join_all(futs).await;
        }
    }

    async fn message_update(
        &self,
        ctx: d::Context,
        _old: Option<d::Message>,
        _new: Option<d::Message>,
        upd: d::MessageUpdateEvent,
    ) {
        if upd.author.is_none() {
            log::error!("Updates without an author id are currently unsupported.");
            return;
        }
        if (&upd.author).as_ref().expect("author is checked").id == ctx.cache.current_user().id {
            return;
        }
        if upd.content.is_none() {
            return;
        }
        let telegram_chat = match db::get_telegram_chat_id(upd.channel_id.clone()) {
            Some(chat_id) => chat_id,
            None => {
                log::info!("Got message {upd:?} in unregistered discord channel");
                return;
            }
        };
        match db::get_telegram_message_id(&self.db, upd.id)
            .await
            .as_deref()
        {
            Ok(&[mirror_id, ..]) => {
                let content = d::content_safe(
                    &ctx,
                    upd.content.as_deref().unwrap_or(""),
                    &d::ContentSafeOptions::default(),
                    &[],
                );
                let text = format::discord_to_telegram_format(&content);

                let mut msg_with_author =
                    ctx.cache.message(upd.channel_id, upd.id).map(|m| m.clone());

                if msg_with_author.is_none() {
                    msg_with_author =
                        discord_request(|| ctx.http.get_message(upd.channel_id, upd.id), || ())
                            .await;
                }

                let author = match msg_with_author {
                    Some(msg) => format::discord_author_name(&ctx, &msg).await,
                    None => "Unknown".into(),
                };
                let mut message_text = format!("<b>{author}</b>\n{text}");
                let mut builder = self
                    .telegram_bot
                    .edit_message_text(telegram_chat, mirror_id, "")
                    .parse_mode(t::ParseMode::Html);

                if let Some(ref_msg_id) = upd
                    .referenced_message
                    .as_ref()
                    .and_then(|m| m.as_ref().map(|m| m.id))
                {
                    let found_mirror = match db::get_telegram_message_id(&self.db, ref_msg_id)
                        .await
                        .as_deref()
                    {
                        Ok([]) => false,
                        Ok(_) => true,
                        Err(e) => {
                            log::error!("Database lookup failed: {e}");
                            false
                        }
                    };
                    if !found_mirror {
                        if let Some(ref_msg) =
                            upd.referenced_message.as_ref().and_then(|m| m.as_ref())
                        {
                            let ref_content = format::discord_to_telegram_format(&ref_msg.content);
                            let ref_author = format::discord_author_name(&ctx, ref_msg).await;
                            message_text = format!("<blockquote expandable><b>{ref_author}</b>\n{ref_content}</blockquote>\n{message_text}");
                        }
                    }
                }
                builder = builder.text(message_text);

                telegram_request!(
                    builder.clone(),
                    log::error!(
                        "{}:{}, Sender: {}\nText: {}\nFormatted: {}",
                        file!(),
                        line!(),
                        author,
                        content,
                        text
                    ),
                )
                .await;
            }
            // the edited message had no known counterpart so do nothing
            Ok([]) => {}
            Err(e) => log::error!("Failed to get message mapping: {}", e),
        }
    }

    async fn message_delete(
        &self,
        _ctx: d::Context,
        channel_id: d::ChannelId,
        msg_id: d::MessageId,
        _guild_id: Option<d::GuildId>,
    ) {
        let telegram_chat = match db::get_telegram_chat_id(channel_id.clone()) {
            Some(chat_id) => chat_id,
            None => {
                log::info!("Got message {msg_id:?} in unregistered discord channel");
                return;
            }
        };
        match db::delete_by_discord(&self.db, msg_id).await {
            Ok(mirror_ids) => {
                for telegram_id in mirror_ids {
                    telegram_request!(self.telegram_bot.delete_message(telegram_chat, telegram_id))
                        .await;
                }
            }
            Err(e) => log::error!("Failed to delete message mapping: {}", e),
        }
    }

    async fn reaction_add(&self, ctx: d::Context, reaction: d::Reaction) {
        let Some(telegram_chat) = db::get_telegram_chat_id(reaction.channel_id) else {
            log::info!("Got reaction {reaction:?} in unregistered discord channel");
            return;
        };
        let d::ReactionType::Unicode(emoji) = &reaction.emoji else {
            log::info!("Got reaction {reaction:?} with non-unicode emoji");
            return;
        };
        let discord_id = reaction.message_id;
        let telegram_id = match db::get_telegram_message_id(&self.db, discord_id)
            .await
            .as_deref()
        {
            Ok(&[telegram_id, ..]) => telegram_id,
            Ok([]) => {
                log::info!("Got reaction {reaction:?} with no known counterpart");
                return;
            }
            Err(e) => {
                log::error!("Failed to get telegram message id: {}", e);
                return;
            }
        };
        match db::get_telegram_reaction_message_id(&self.db, discord_id).await {
            Ok(Some((reaction_message_id, reactions))) => {
                // Reaction message exists, update it
                let mut reactions = format::parse_telegram_reaction_message(&reactions);
                let author = format::discord_reactor_name(&ctx, &reaction).await;
                reactions.entry(author).or_default().push(emoji.clone());

                let new_text = format::format_telegram_reaction_message(&reactions);

                if let Some(_) = telegram_request!(self
                    .telegram_bot
                    .edit_message_text(telegram_chat, reaction_message_id, &new_text)
                    .parse_mode(t::ParseMode::Html))
                .await
                {
                    if let Err(e) =
                        db::update_telegram_reaction_mapping(&self.db, discord_id, &new_text).await
                    {
                        log::error!("Failed to update reaction message mapping: {}", e);
                    }
                }
            }
            _ => {
                // Reaction message doesn't exist, create a new one
                let author = format::discord_reactor_name(&ctx, &reaction).await;
                let text = format!("<b>Reactions</b>\n<b>{}</b>: {}", author, emoji);

                if let Some(telegram_msg) = telegram_request!(self
                    .telegram_bot
                    .send_message(telegram_chat, &text)
                    .parse_mode(t::ParseMode::Html)
                    .reply_to_message_id(telegram_id))
                .await
                {
                    if let Err(e) = db::insert_reaction_mapping(
                        &self.db,
                        discord_id,
                        telegram_msg.id,
                        telegram_chat,
                        &text,
                    )
                    .await
                    {
                        log::error!("Failed to insert reaction message mapping: {}", e);
                    }
                }
            }
        }
    }

    async fn reaction_remove(&self, ctx: d::Context, reaction: d::Reaction) {
        let Some(telegram_chat) = db::get_telegram_chat_id(reaction.channel_id) else {
            log::info!("Got reaction {reaction:?} in unregistered discord channel");
            return;
        };
        let d::ReactionType::Unicode(emoji) = &reaction.emoji else {
            log::info!("Got reaction {reaction:?} with non-unicode emoji");
            return;
        };
        let discord_id = reaction.message_id;
        let author = format::discord_reactor_name(&ctx, &reaction).await;
        match db::get_telegram_reaction_message_id(&self.db, discord_id).await {
            Ok(Some((telegram_id, reactions))) => {
                let mut reactions = format::parse_telegram_reaction_message(&reactions);
                reactions.entry(author).or_default().retain(|e| e != emoji);
                let new_text = format::format_telegram_reaction_message(&reactions);
                if new_text == "<b>Reactions</b>" {
                    if let Some(_) = telegram_request!(self
                        .telegram_bot
                        .delete_message(telegram_chat, telegram_id))
                    .await
                    {
                        if let Err(e) =
                            db::remove_reaction_mapping_by_discord(&self.db, discord_id).await
                        {
                            log::error!("Failed to remove reaction message mapping: {}", e);
                        }
                    }
                } else {
                    if let Some(_) = telegram_request!(self
                        .telegram_bot
                        .edit_message_text(telegram_chat, telegram_id, &new_text)
                        .parse_mode(t::ParseMode::Html))
                    .await
                    {
                        if let Err(e) =
                            db::update_telegram_reaction_mapping(&self.db, discord_id, &new_text)
                                .await
                        {
                            log::error!("Failed to update reaction message mapping: {}", e);
                        }
                    }
                }
            }
            Ok(None) => {
                log::info!("Got reaction removal {reaction:?} with no known counterpart");
            }
            Err(e) => log::error!("Failed to get reaction message mapping: {}", e),
        }
    }

    // Set a handler to be called on the `ready` event. This is called when a
    // shard is booted, and a READY payload is sent by Discord. This payload
    // contains data like the current user's guild Ids, current user data,
    // private channels, and more.
    //
    // In this case, just print what the current user's username is.
    async fn ready(&self, _ctx: d::Context, ready: d::Ready) {
        log::warn!("{} is connected!", ready.user.name);
    }
}

#[rustfmt::skip]
async fn get_telegram_attachment_as_discord(bot: &t::Bot, msg: &t::Message) -> Option<d::CreateAttachment> {
    if let (
        Some(t::Document {file, file_name, .. }),_,_,_,_,_,_,
    ) | (
        _, Some([.., t::PhotoSize { file, .. }]),_,_,_,_, file_name
    ) | (
        _,_, Some(t::Video { file, file_name, .. }),_,_,_,_,
    ) | (
        _,_,_, Some(t::Audio { file, file_name, .. }),_,_,_,
    ) | (
        _,_,_,_, Some(t::Animation { file, file_name, .. }),_,_,
    ) | (
        _,_,_,_,_, Some(t::Sticker {file, set_name: file_name, .. }),_,
    ) = (
        msg.document(), msg.photo(), msg.video(), msg.audio(), msg.animation(), msg.sticker(), &None,
    ) {
        if file.size > 50 * 1024 * 1024 {
            telegram_request!(bot
                .send_message(
                    msg.chat.id,
                    "File too large. Files over 50 MB won't be forwarded"
                )
                .reply_to_message_id(msg.id)
                .clone())
            .await;
            return None;
        }
        let file = match bot.get_file(file.id.as_str()).await {
            Ok(file) => file,
            Err(e) => {
                log::error!("Failed to get file: {e:?}");
                return None;
            }
        };
        let path = file.path;
        let Some(mut name) = file_name
            .clone()
            .or_else(|| path.split('/').last().map(String::from))
        else {
            log::error!("Failed to get file name");
            return None;
        };
        if msg.has_media_spoiler() {
            name = format!("SPOILER_{name}");
        }
        let mut bytes = Vec::new();
        let mut retries = RETRIES;
        let mut backoff = INITIAL_BACKOFF;
        while let Err(e) = bot.download_file(&path, &mut bytes).await {
            match e {
                t::DownloadError::Network(_) if retries > 0 => {
                    tokio::time::sleep(backoff).await;
                    backoff *= 2;
                    retries -= 1;
                }
                _ => {
                    log::error!("Failed to download file: {e:?}");
                    break;
                }
            }
        }
        Some(d::CreateAttachment::bytes(bytes, name))
    } else {
        None
    }
}

async fn handle_update(
    bot: t::Bot,
    upd: t::Update,
    discord_http: Arc<d::Http>,
    db_pool: SqlitePool,
) -> Result<(), eyre::Report> {
    log::info!("{upd:?}");

    let Some(telegram_chat) = upd.chat().cloned() else {
        log::error!("Got update {upd:?} without a chat");
        return Ok(());
    };
    let discord_chat = match db::get_discord_channel_id(telegram_chat.id) {
        Some(chat_id) => chat_id,
        None => {
            log::info!("Got message {upd:?} in unregistered telegram chat");
            return Ok(());
        }
    };

    match upd.kind {
        t::UpdateKind::Message(msg) => {
            let author = format::telegram_author_name(&msg);
            let text = msg
                .text()
                .zip(msg.parse_entities())
                .or_else(|| msg.caption().zip(msg.parse_caption_entities()))
                .map(|(t, e)| format::telegram_to_discord_format(t, e));
            let mut content = format!("**{author}**\n{}", text.as_deref().unwrap_or(""));
            let mut message = d::CreateMessage::new();

            if let Some(ref_msg) = msg.reply_to_message() {
                let found_mirror =
                    match db::get_discord_message_id(&db_pool, ref_msg.id, telegram_chat.id)
                        .await
                        .as_deref()
                    {
                        Ok(&[mirror_id, ..]) => {
                            message = message.reference_message((discord_chat, mirror_id));
                            true
                        }
                        Ok([]) => false,
                        Err(e) => {
                            log::error!("Database lookup failed: {e}");
                            false
                        }
                    };
                if !found_mirror {
                    let ref_text = ref_msg.text().unwrap_or("").replace("\n", "\n> ");
                    let ref_author = format::telegram_author_name(ref_msg);
                    content = format!("> **{ref_author}**\n> {ref_text}\n{content}");
                }
            }
            message = message.content(&content);

            if let Some(attachment) = get_telegram_attachment_as_discord(&bot, &msg).await {
                message = message.add_file(attachment);
            }

            let discord_result = discord_request!(
                discord_chat.send_message(discord_http.clone(), message.clone()),
                log::error!(
                    "{}:{} Sender: {}\nText: {}\nFormatted: {}",
                    file!(),
                    line!(),
                    author,
                    msg.text().unwrap_or(""),
                    content
                )
            )
            .await;

            if let Some(discord_msg) = discord_result {
                if let Err(e) =
                    db::insert_mapping(&db_pool, discord_msg.id, msg.id, telegram_chat.id).await
                {
                    log::error!("Failed to insert message mapping: {}", e);
                }
            }
        }
        t::UpdateKind::EditedMessage(msg) => {
            match db::get_discord_message_id(&db_pool, msg.id, telegram_chat.id)
                .await
                .as_deref()
            {
                Ok(&[mirror_id, ..]) => {
                    let text = msg
                        .text()
                        .zip(msg.parse_entities())
                        .or_else(|| msg.caption().zip(msg.parse_caption_entities()))
                        .map(|(t, e)| format::telegram_to_discord_format(t, e));
                    if let Some(".") = text.as_deref() {
                        telegram_request!(bot.delete_message(msg.chat.id, msg.id)).await;
                        discord_request!(
                            discord_chat.delete_message(discord_http.clone(), mirror_id)
                        )
                        .await;
                        return Ok(());
                    }
                    let author = format::telegram_author_name(&msg);
                    let mut content = format!("**{author}**\n{}", text.as_deref().unwrap_or(""));
                    let mut message = d::EditMessage::new();

                    if let Some(ref_msg) = msg.reply_to_message() {
                        let found_mirror = match db::get_discord_message_id(
                            &db_pool,
                            ref_msg.id,
                            telegram_chat.id,
                        )
                        .await
                        .as_deref()
                        {
                            Ok([]) => false,
                            Ok(_) => true,
                            Err(e) => {
                                log::error!("Database lookup failed: {e}");
                                false
                            }
                        };
                        if !found_mirror {
                            let ref_text = ref_msg.text().unwrap_or("").replace("\n", "\n> ");
                            let ref_author = format::telegram_author_name(ref_msg);
                            content = format!("> **{ref_author}**\n> {ref_text}\n{content}");
                        }
                    }
                    message = message.content(&content);

                    discord_request!(
                        discord_chat.edit_message(discord_http.clone(), mirror_id, message.clone()),
                        log::error!(
                            "{}:{} Sender: {}\nText: {}\nFormatted: {}",
                            file!(),
                            line!(),
                            author,
                            msg.text().unwrap_or(""),
                            content
                        ),
                    )
                    .await;
                }
                // the edited message had no known counterpart
                // if the edit is a ., delete it for consistency
                Ok([]) => {
                    if let (Some("."), _) | (_, Some(".")) =
                        (msg.text().as_deref(), msg.caption().as_deref())
                    {
                        telegram_request!(bot.delete_message(msg.chat.id, msg.id)).await;
                        return Ok(());
                    }
                }
                Err(e) => log::error!("Failed to get message mapping: {}", e),
            }
        }
        _ => {}
    }
    Ok(())
}

const RETRIES: usize = 5;
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);

async fn telegram_request<F, Log, Fut, T: Send + Sync>(mut f: F, log: Log) -> Option<T>
where
    Fut: IntoFuture<Output = Result<T, t::RequestError>>,
    Log: Fn(),
    F: FnMut() -> Fut,
{
    let mut retries: usize = RETRIES;
    let mut backoff: Duration = INITIAL_BACKOFF;
    loop {
        match f().await {
            Ok(x) => break Some(x),
            Err(t::RequestError::RetryAfter(d)) => tokio::time::sleep(d.duration()).await,
            Err(why @ t::RequestError::Network(_)) => {
                if retries == 0 {
                    log::error!("Failed telegram request: {why:?}");
                    log();
                    break None;
                } else {
                    log::warn!(
                        "Failed telegram request: {why:?}. Retrying {retries} more times..."
                    );
                    tokio::time::sleep(backoff).await;
                    retries -= 1;
                    backoff *= 2;
                }
            }
            Err(why) => {
                log::error!("Failed telegram request: {why:?}");
                log();
                break None;
            }
        }
    }
}

async fn discord_request<F, Log, Fut, T: Send + Sync>(mut f: F, log: Log) -> Option<T>
where
    Fut: IntoFuture<Output = Result<T, d::Error>>,
    Log: Fn(),
    F: FnMut() -> Fut,
{
    let mut retries: usize = RETRIES;
    let mut backoff: Duration = INITIAL_BACKOFF;
    loop {
        match f().await {
            Ok(x) => break Some(x),
            Err(why @ (d::Error::Gateway(_) | d::Error::Http(_))) => {
                if retries == 0 {
                    log::error!("Failed discord request: {why:?}");
                    log();
                    break None;
                } else {
                    log::warn!("Failed discord request: {why:?}. Retrying {retries} more times...");
                    tokio::time::sleep(backoff).await;
                    backoff *= 2;
                    retries -= 1;
                }
            }
            Err(why) => {
                log::error!("Failed discord request: {why:?}");
                log();
                break None;
            }
        }
    }
}

#[tokio::main]
async fn main() {
    dotenv::dotenv().ok();
    pretty_env_logger::init();

    let db_pool = db::init_db().await.expect("Failed to initialize database");

    let telegram_bot = t::Bot::from_env();

    // Configure the client with your Discord bot token in the environment.
    let discord_token = env::var(DISCORD_TOKEN_ENV).expect("Expected a token in the environment");
    // Set gateway intents, which decides what events the bot will be notified about
    let intents = GatewayIntents::all();

    // Create a new instance of the Client, logging in as a bot. This will
    // automatically prepend your bot token with "Bot ", which is a requirement
    // by Discord for bot users.
    let mut discord_client = Client::builder(&discord_token, intents)
        .event_handler(DiscordState {
            telegram_bot: telegram_bot.clone(),
            db: db_pool.clone(),
        })
        .await
        .expect("Err creating client");

    let discord_http = discord_client.http.clone();

    log::warn!("Starting telegram...");

    let telegram_handler = endpoint(handle_update);

    let mut telegram_dispatch = Dispatcher::builder(telegram_bot, telegram_handler)
        .dependencies(dptree::deps![discord_http, db_pool])
        .enable_ctrlc_handler()
        .build();

    let _telegram_handle = tokio::spawn(async move { telegram_dispatch.dispatch().await });

    // Finally, start a single shard, and start listening to events.
    //
    // Shards will automatically attempt to reconnect, and will perform
    // exponential backoff until it reconnects.
    let _discord_handle = tokio::spawn(async move {
        discord_client.start().await.expect("Discord start failed");
    });

    let _ = tokio::signal::ctrl_c().await;
    std::process::exit(1);
}
