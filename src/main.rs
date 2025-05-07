#![allow(dead_code)]
mod db;
mod format;

use dashmap::DashMap;
use sqlx::SqlitePool;
use tokio::time::Instant;

use std::{env, future::IntoFuture, sync::Arc, time::Duration};

// use tokio::sync::RwLock;
mod telegram {
    pub use teloxide::errors::{DownloadError, RequestError};
    pub use teloxide::prelude::*;
    pub use teloxide::types::*;
}

use teloxide::{net::Download as _, prelude::*};
use url::Url;

use dotenv;

#[allow(unused_imports)]
mod discord {
    pub use serenity::{
        all::{
            content_safe, ContentSafeOptions, Permissions, ResolvedOption, ResolvedValue, UserId,
        },
        async_trait,
        builder::{
            AutocompleteChoice, CreateAllowedMentions, CreateAttachment,
            CreateAutocompleteResponse, CreateChannel, CreateCommand, CreateCommandOption,
            CreateEmbed, CreateEmbedAuthor, CreateInteractionResponse,
            CreateInteractionResponseMessage, CreateMessage, CreateWebhook, EditMessage,
            EditWebhookMessage, ExecuteWebhook,
        },
        cache::Cache,
        http::Http,
        model::{
            application::{
                Command, CommandInteraction, CommandOptionType, Interaction, InteractionContext,
            },
            channel::{Attachment, ChannelType, Message, Reaction, ReactionType},
            event::MessageUpdateEvent,
            gateway::Ready,
            id::{ChannelId, GuildId, MessageId},
            sticker::{StickerFormatType, StickerItem},
            webhook::{Webhook, WebhookChannel, WebhookGuild, WebhookType},
        },
        prelude::*,
        Error,
    };
}
use serenity::prelude::Mentionable as _;

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

macro_rules! edbg {
    ($($e:expr),*$(,)?) => {
        log::error!(concat!("{}:{}\n", $(concat!(stringify!($e), ": {:?}\n")),*), file!(), line!(), $($e),*)
    };
}

impl DiscordState {
    async fn send_message(
        &self,
        telegram_chat: t::ChatId,
        msg_id: d::MessageId,
        text: &str,
        attachments: Vec<d::Attachment>,
        mut stickers: Vec<d::StickerItem>,
        reply_to_message_id: Option<t::MessageId>,
    ) {
        enum AttachmentKind {
            Image,
            Video,
            Audio,
            Other,
        }
        use AttachmentKind as AK;

        fn kind(a: &d::Attachment) -> AttachmentKind {
            let Some(ct) = a.content_type.as_deref() else {
                return AK::Other;
            };
            if ct.starts_with("video/") || ct == "image/gif" {
                return AK::Video;
            } else if ct.starts_with("image/") {
                return AK::Image;
            } else if ct.starts_with("audio/") {
                return AK::Audio;
            } else {
                return AK::Other;
            }
        }

        fn skind(s: &d::StickerItem) -> AttachmentKind {
            match s.format_type {
                d::StickerFormatType::Gif | d::StickerFormatType::Apng => AK::Video,
                d::StickerFormatType::Png => AK::Image,
                _ => AK::Other,
            }
        }

        macro_rules! _if_method_may_spoiler {
            (send_photo, $code:expr) => {
                $code
            };
            (send_video, $code:expr) => {
                $code
            };
            ($method:ident, $code:expr) => {};
        }

        macro_rules! _send_with_method {
            ($method:ident, $filename:expr, $url:expr, $caption:expr, $replyto:expr $(,)?) => {
                async {
                    #[allow(unused_mut)]
                    let mut f = t::InputFile::url($url.clone());
                    _if_method_may_spoiler!($method, {
                        f = f.file_name($filename.trim_start_matches("SPOILER_").to_string());
                    });
                    let mut s = self.telegram_bot.$method(telegram_chat, f);
                    _if_method_may_spoiler!($method, {
                        s = s.has_spoiler($filename.starts_with("SPOILER_"));
                    });
                    if let Some(caption) = $caption {
                        s = s.caption(caption).parse_mode(t::ParseMode::Html);
                    }
                    if let Some(id) = $replyto {
                        s = s.reply_parameters(t::ReplyParameters::new(id));
                    }
                    telegram_request!(s.send_ref()).await
                }
            };
        }

        macro_rules! send_with_attachment {
            ($a:expr, $caption:expr, $replyto:expr $(,)?) => {
                async {
                    let a: &d::Attachment = $a;
                    let caption: Option<&str> = $caption;
                    let replyto: Option<t::MessageId> = $replyto;
                    let url = match Url::parse(a.url.as_str()) {
                        Ok(url) => url,
                        Err(e) => {
                            log::error!("Failed to parse attachment url: {e}");
                            return None;
                        }
                    };
                    match kind(a) {
                        AK::Image => {
                            _send_with_method!(send_photo, a.filename, url, caption, replyto).await
                        }
                        AK::Video => {
                            _send_with_method!(send_video, a.filename, url, caption, replyto).await
                        }
                        AK::Audio => {
                            _send_with_method!(send_audio, a.filename, url, caption, replyto).await
                        }
                        AK::Other => {
                            _send_with_method!(send_document, a.filename, url, caption, replyto)
                                .await
                        }
                    }
                }
            };
        }

        macro_rules! send_with_sticker {
            ($s:expr, $caption:expr, $replyto:expr $(,)?) => {
                async {
                    let s: &d::StickerItem = $s;
                    let caption: Option<&str> = $caption;
                    let replyto: Option<t::MessageId> = $replyto;
                    let Some(url) = s.image_url() else {
                        log::error!("Failed to get sticker url: {s:?}");
                        return None;
                    };
                    let url = match Url::parse(url.as_str()) {
                        Ok(url) => url,
                        Err(e) => {
                            log::error!("Failed to parse attachment url: {e}");
                            return None;
                        }
                    };
                    let filename = url
                        .path_segments()
                        .and_then(Iterator::last)
                        .unwrap_or("sticker");
                    match skind(s) {
                        AK::Video => {
                            _send_with_method!(send_video, filename, url, caption, replyto).await
                        }
                        AK::Image => {
                            _send_with_method!(send_photo, filename, url, caption, replyto).await
                        }
                        _ => None,
                    }
                }
            };
        }

        stickers.retain(|s| !matches!(skind(s), AK::Other));

        let mut attachments_processed = false;
        let total_attachments = attachments.len() + stickers.len();
        let media_count = attachments
            .iter()
            .filter(|a| matches!(kind(a), AK::Image | AK::Video))
            .count()
            + stickers.len();

        let telegram_result: Vec<_> = if total_attachments == 1 {
            attachments_processed = true;
            if stickers.is_empty() {
                send_with_attachment!(&attachments[0], Some(&text), reply_to_message_id)
                    .await
                    .into_iter()
                    .collect()
            } else {
                send_with_sticker!(&stickers[0], Some(&text), reply_to_message_id)
                    .await
                    .into_iter()
                    .collect()
            }
        } else if media_count >= 1 {
            attachments_processed = total_attachments == media_count;
            let mut builder = self.telegram_bot.send_media_group(
                telegram_chat,
                attachments
                    .iter()
                    .filter(|a| matches!(kind(a), AK::Image | AK::Video))
                    .map(|a| a.url.as_str())
                    .map(url::Url::parse)
                    .filter_map(|url| {
                        url.inspect_err(|e| log::error!("Failed to parse attachment url: {e}"))
                            .ok()
                    })
                    .map(t::InputFile::url)
                    .zip(attachments.iter())
                    .map(|(m, a)| {
                        (
                            m.file_name(a.filename.trim_start_matches("SPOILER_").to_string()),
                            a.filename.starts_with("SPOILER_"),
                            kind(a),
                        )
                    })
                    .chain(stickers.iter().filter_map(|s| {
                        let Some(url) = s.image_url() else {
                            return None;
                        };
                        let url = match Url::parse(url.as_str()) {
                            Ok(url) => url,
                            Err(e) => {
                                log::error!("Failed to parse attachment url: {e}");
                                return None;
                            }
                        };
                        let filename = url
                            .path_segments()
                            .and_then(Iterator::last)
                            .unwrap_or("sticker")
                            .to_string();
                        Some((t::InputFile::url(url).file_name(filename), false, skind(s)))
                    }))
                    .enumerate()
                    .map(|(i, (m, s, k))| match k {
                        AK::Image => t::InputMedia::Photo({
                            let mut m = t::InputMediaPhoto::new(m);
                            if i == 0 {
                                m = m.caption(text).parse_mode(t::ParseMode::Html);
                            }
                            if s {
                                m = m.spoiler();
                            }
                            m
                        }),
                        AK::Video => t::InputMedia::Video({
                            let mut m = t::InputMediaVideo::new(m);
                            if i == 0 {
                                m = m.caption(text).parse_mode(t::ParseMode::Html);
                            }
                            if s {
                                m = m.spoiler();
                            }
                            m
                        }),
                        _ => unreachable!(),
                    })
                    .collect::<Vec<_>>(),
            );
            if let Some(id) = reply_to_message_id {
                builder = builder.reply_parameters(t::ReplyParameters::new(id));
            }
            telegram_request!(builder.send_ref())
                .await
                .into_iter()
                .flatten()
                .collect()
        } else {
            vec![]
        };
        let telegram_result = if telegram_result.is_empty() {
            let builder = self
                .telegram_bot
                .send_message(telegram_chat, text)
                .parse_mode(t::ParseMode::Html);
            let s = if let Some(id) = reply_to_message_id {
                builder.reply_parameters(t::ReplyParameters::new(id))
            } else {
                builder
            };

            telegram_request!(s.send_ref(), edbg!(text),)
                .await
                .into_iter()
                .collect()
        } else {
            telegram_result
        };

        for telegram_msg in telegram_result {
            if let Err(e) = db::insert_mapping(
                &self.db,
                msg_id,
                telegram_msg.id,
                telegram_chat,
                telegram_msg.caption().is_some(),
            )
            .await
            {
                log::error!("Failed to insert message mapping: {}", e);
            }
        }

        if attachments_processed {
            return;
        }

        let att_futs = attachments
            .into_iter()
            .map(|a| async move {
                if let Some(telegram_msg) = send_with_attachment!(&a, None, None).await {
                    if let Err(e) =
                        db::insert_mapping(&self.db, msg_id, telegram_msg.id, telegram_chat, true)
                            .await
                    {
                        log::error!("Failed to insert message mapping: {}", e);
                    }
                }
            })
            .collect::<Vec<_>>();
        let stick_futs = stickers
            .into_iter()
            .map(|s| async move {
                if let Some(telegram_msg) = send_with_sticker!(&s, None, None).await {
                    if let Err(e) =
                        db::insert_mapping(&self.db, msg_id, telegram_msg.id, telegram_chat, true)
                            .await
                    {
                        log::error!("Failed to insert message mapping: {}", e);
                    }
                }
            })
            .collect::<Vec<_>>();
        futures::future::join_all(att_futs).await;
        futures::future::join_all(stick_futs).await;
    }

    async fn get_available_telegram_chats(&self) -> Vec<(t::ChatId, String)> {
        db::get_telegram_chats(&self.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|&(id, _)| db::get_discord_channel_id(id).is_none())
            .collect()
    }

    async fn register_commands(&self, http: &Arc<d::Http>) -> Result<(), d::Error> {
        discord_request!(d::Command::create_global_command(
            http,
            d::CreateCommand::new("bridge")
                .description("Bridge a Telegram chat to this Discord channel.")
                .default_member_permissions(d::Permissions::MANAGE_CHANNELS)
                .add_option(
                    d::CreateCommandOption::new(
                        d::CommandOptionType::Integer,
                        "chat",
                        "The Telegram chat to bridge.",
                    )
                    .required(true)
                    .set_autocomplete(true),
                ),
        ))
        .await;
        discord_request!(d::Command::create_global_command(
            http,
            d::CreateCommand::new("unbridge")
                .description("Remove the Telegram chat bridge in this Discord channel.")
                .default_member_permissions(d::Permissions::MANAGE_CHANNELS),
        ))
        .await;
        discord_request!(d::Command::create_global_command(
            http,
            d::CreateCommand::new("hub")
                .description("Make a category into a named hub. WARNING: See `/hubinfo`.")
                .default_member_permissions(d::Permissions::MANAGE_CHANNELS)
                .add_option(
                    d::CreateCommandOption::new(
                        d::CommandOptionType::String,
                        "hub_name",
                        "The name of the hub. It should not contain any whitespace.",
                    )
                    .required(true),
                )
                .add_option(
                    d::CreateCommandOption::new(
                        d::CommandOptionType::Channel,
                        "hub_category",
                        "Category for hub channels. Omit to make channels uncategorized."
                    )
                    .required(false)
                    .set_autocomplete(true)
                    .channel_types(vec![d::ChannelType::Category])
                )
                .add_context(d::InteractionContext::Guild),
        ))
        .await;
        discord_request!(d::Command::create_global_command(
            http,
            d::CreateCommand::new("unhub")
                .description("Remove hub status from the server or a category in it. This will not remove any bridges.")
                .default_member_permissions(d::Permissions::MANAGE_CHANNELS)
                .add_option(
                    d::CreateCommandOption::new(
                        d::CommandOptionType::String,
                        "hub_name",
                        "The name of the hub to remove.",
                    )
                    .required(true)
                    .set_autocomplete(true),
                ).add_context(d::InteractionContext::Guild),
        ))
        .await;
        discord_request!(d::Command::create_global_command(
            http,
            d::CreateCommand::new("hubinfo").description("Provides info about the hub feature."),
        ))
        .await;
        Ok(())
    }

    async fn handle_bridge_command(&self, ctx: &d::Context, command: &d::CommandInteraction) {
        macro_rules! reply {
            (internal: $r:expr, $ephem:expr) => {{
                let r = $r;
                let t: &str = r.as_ref();
                discord_request!(command.create_response(
                    &ctx.http,
                    d::CreateInteractionResponse::Message(
                        d::CreateInteractionResponseMessage::new()
                            .content(t)
                            .ephemeral($ephem),
                    ),
                ))
                .await;
            }};
            ($r:expr $(,)?) => {reply!(internal: $r, false)};
            (ephemeral: $r:expr $(,)?) => {
                reply!(internal: $r, true)
            };
        }

        if let Some(existing_telegram_channel) = db::get_telegram_chat_id(command.channel_id) {
            reply!(ephemeral: format!(
                "This Discord channel is already bridged to a Telegram chat: <#{}>.\nUse /unbridge to remove that bridge.",
                existing_telegram_channel
            ));
            return;
        }

        let Some(chat_id) = command
            .data
            .options
            .get(0)
            .and_then(|opt| opt.value.as_i64())
        else {
            reply!(ephemeral: "Invalid chat selected.");
            return;
        };

        let telegram_chat_id = t::ChatId(chat_id);

        // Check if this Telegram chat is already mapped to a Discord channel
        if let Some((existing_discord_channel, _)) = db::get_discord_channel_id(telegram_chat_id) {
            reply!(ephemeral: format!(
                "This Telegram chat is already bridged to another Discord channel: <#{}>",
                existing_discord_channel
            ));
            return;
        }

        // Verify that the bot is a member of the chat by trying to get chat info
        let get_chat_result = telegram_request!(
            self.telegram_bot.get_chat(telegram_chat_id),
            log::error!("Failed to get chat info for {}", telegram_chat_id.0)
        )
        .await;

        let Some(chat) = get_chat_result else {
            // Bot is not a member of the chat, mark it as not a member in the database
            if let Err(e) =
                db::update_chat_membership(&self.db, telegram_chat_id, "Unknown chat", false).await
            {
                log::error!("Failed to update chat membership: {}", e);
            }
            reply!(ephemeral: "The bot is not a member of this Telegram chat. Please add the bot to the chat first.");
            return;
        };
        let title = chat
            .title()
            .or_else(|| chat.username())
            .unwrap_or("unknown chat")
            .to_string();
        match db::set_chat_mapping(command.channel_id, t::ChatId(chat_id), None).await {
            Ok(()) => reply!(format!(
                "Successfully bridged Telegram chat \"{title}\" to this channel!"
            )),
            Err(e) => {
                log::error!("Failed to set chat mapping: {}", e);
                reply!(ephemeral: "Failed to bridge chat. Please try again later.");
                return;
            }
        }

        let telegram_notification = self.telegram_bot.send_message(
            telegram_chat_id,
            format!(
                "A bridge has been created to the Discord channel \"{}\".",
                command
                    .channel
                    .as_ref()
                    .and_then(|c| c.name.as_deref())
                    .unwrap_or("[name unknown]")
            ),
        );
        telegram_request!(telegram_notification.send_ref()).await;
    }

    async fn handle_unbridge_command(&self, ctx: &d::Context, command: &d::CommandInteraction) {
        macro_rules! reply {
            (internal: $r:expr, $ephem:expr) => {{
                let r = $r;
                let t: &str = r.as_ref();
                discord_request!(command.create_response(
                    &ctx.http,
                    d::CreateInteractionResponse::Message(
                        d::CreateInteractionResponseMessage::new()
                            .content(t)
                            .ephemeral($ephem),
                    ),
                ))
                .await;
            }};
            ($r:expr $(,)?) => {reply!(internal: $r, false)};
            (ephemeral: $r:expr $(,)?) => {
                reply!(internal: $r, true)
            };
        }
        // Check if the channel is currently bridged
        let Some(telegram_chat_id) = db::get_telegram_chat_id(command.channel_id) else {
            reply!(ephemeral: "This channel is not currently bridged to any Telegram chat.");
            return;
        };
        // Remove the bridge mapping
        match db::remove_chat_mapping(db::RemovalChatId::Discord(command.channel_id)).await {
            Ok(()) => {
                reply!("Successfully unbridged this channel from Telegram.");
                let telegram_notification = self.telegram_bot.send_message(
                    telegram_chat_id,
                    "The bridge to this channel has been removed.",
                );
                telegram_request!(telegram_notification.send_ref()).await;
            }
            Err(e) => {
                log::error!("Failed to remove chat mapping: {}", e);
                reply!(ephemeral: "Failed to unbridge channel. Please try again later.");
            }
        }
    }

    async fn handle_hub_command(&self, ctx: &d::Context, command: &d::CommandInteraction) {
        macro_rules! reply {
            (internal: $r:expr, $ephem:expr) => {{
                let r = $r;
                let t: &str = r.as_ref();
                discord_request!(command.create_response(
                    &ctx.http,
                    d::CreateInteractionResponse::Message(
                        d::CreateInteractionResponseMessage::new()
                            .content(t)
                            .ephemeral($ephem),
                    ),
                ))
                .await;
            }};
            ($r:expr $(,)?) => {reply!(internal: $r, false)};
            (ephemeral: $r:expr $(,)?) => {
                reply!(internal: $r, true)
            };
        }
        let Some(guild_id) = command.guild_id else {
            reply!(ephemeral: "Only servers (and not DMs or group DMs) can be made into hubs.");
            return;
        };
        let Some(name) = command.data.options.get(0).and_then(|n| n.value.as_str()) else {
            reply!(ephemeral: "Expected a name for the hub.");
            return;
        };
        if name.contains(char::is_whitespace) || name == "" {
            reply!(ephemeral: "Name cannot contain whitespace or be empty.");
            return;
        }
        let category = match command.data.options().get(1) {
            Some(d::ResolvedOption {
                name: "hub_category",
                value: d::ResolvedValue::Channel(c),
                ..
            }) => {
                if !matches!(c.kind, d::ChannelType::Category) {
                    reply!(ephemeral: "Hub category argument should be a category.");
                    return;
                }
                Some(c.id)
            }
            Some(d::ResolvedOption {
                name: "hub_category",
                ..
            }) => {
                reply!(ephemeral: "Hub category argument should be a category.");
                return;
            }
            _ => None,
        };
        let hub = match category {
            Some(c) => db::Hub::Category(guild_id, c),
            None => db::Hub::Server(guild_id),
        };
        match db::add_hub_server(name.to_string(), hub).await {
            Ok(true) => reply!(format!(
                "Successfully created hub named \"{name}\"! Use `/unhub` to undo."
            )),
            Ok(false) => {
                reply!(ephemeral: format!("The name \"{name}\" is taken, try again with another name."))
            }
            Err(e) => {
                log::error!("Failed to add hub with error {e}");
                reply!(ephemeral: "Hub creation failed due to internal error. Please try again later.");
            }
        }
    }
    async fn handle_unhub_command(&self, ctx: &d::Context, command: &d::CommandInteraction) {
        macro_rules! reply {
            (internal: $r:expr, $ephem:expr) => {{
                let r = $r;
                let t: &str = r.as_ref();
                discord_request!(command.create_response(
                    &ctx.http,
                    d::CreateInteractionResponse::Message(
                        d::CreateInteractionResponseMessage::new()
                            .content(t)
                            .ephemeral($ephem),
                    ),
                ))
                .await;
            }};
            ($r:expr $(,)?) => {reply!(internal: $r, false)};
            (ephemeral: $r:expr $(,)?) => {
                reply!(internal: $r, true)
            };
        }
        let Some(guild_id) = command.guild_id else {
            reply!(ephemeral: "Only servers (and not DMs or group DMs) can have hubs.");
            return;
        };
        let Some(name) = command.data.options.get(0).and_then(|n| n.value.as_str()) else {
            reply!(ephemeral: "Expected a name for the hub to remove.");
            return;
        };
        if name.contains(char::is_whitespace) || name == "" {
            reply!(ephemeral: "Name cannot contain whitespace or be empty.");
            return;
        }
        match db::remove_hub_server(name, guild_id).await {
            Ok(Some(db::Hub::Category(_, c))) => reply!(format!(
                "Successfully removed hub named \"{name}\" for category <#{}>!",
                u64::from(c)
            )),
            Ok(Some(db::Hub::Server(_))) => {
                reply!(format!(
                    "Successfully removed hub named \"{name}\" for this server!",
                ))
            }
            Ok(None) => {
                reply!(ephemeral: format!("No hub named \"{name}\" found for your server."))
            }
            Err(e) => {
                log::error!("Failed to add hub with error {e}");
                reply!(ephemeral: "Hub creation failed due to internal error. Please try again later.");
            }
        }
    }
    async fn handle_bridge_autocomplete(
        &self,
        ctx: &d::Context,
        autocomplete: &d::CommandInteraction,
    ) {
        let choices = if db::admins().await.contains(&autocomplete.user.id) {
            self.get_available_telegram_chats()
                .await
                .into_iter()
                // .filter(|chat| chat.title.to_lowercase().contains(&input))
                .take(25)
                .map(|(id, title)| {
                    d::AutocompleteChoice::new(format!("{} ({})", title, id.0), id.0)
                })
                .collect::<Vec<_>>()
        } else {
            vec![]
        };

        if let Err(e) = ctx
            .http
            .create_interaction_response(
                autocomplete.id,
                &autocomplete.token,
                &d::CreateInteractionResponse::Autocomplete(
                    d::CreateAutocompleteResponse::new().set_choices(choices),
                ),
                vec![],
            )
            .await
        {
            log::error!("Failed to respond to autocomplete: {}", e);
        }
    }
    async fn handle_unhub_autocomplete(
        &self,
        ctx: &d::Context,
        autocomplete: &d::CommandInteraction,
    ) {
        let Some(guild_id) = autocomplete.guild_id else {
            return;
        };
        let Some(auto) = autocomplete.data.autocomplete() else {
            return;
        };
        let choices = db::hubs_for_server(guild_id);
        let choices = choices
            .into_iter()
            .filter(|c| c.0.starts_with(auto.value))
            .map(|(name, hub)| {
                d::AutocompleteChoice::new(
                    match hub {
                        db::Hub::Category(_, c) => format!("{name} <#{}>", u64::from(c)),
                        db::Hub::Server(_) => name.clone(),
                    },
                    name,
                )
            })
            .collect::<Vec<_>>();
        if let Err(e) = ctx
            .http
            .create_interaction_response(
                autocomplete.id,
                &autocomplete.token,
                &d::CreateInteractionResponse::Autocomplete(
                    d::CreateAutocompleteResponse::new().set_choices(choices),
                ),
                vec![],
            )
            .await
        {
            log::error!("Failed to respond to autocomplete: {}", e);
        }
    }
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
        if msg.webhook_id.is_some() {
            return;
        }
        let telegram_chat = match db::get_telegram_chat_id(msg.channel_id) {
            Some(chat_id) => chat_id,
            None => {
                log::info!("Got message {msg:?} in unregistered discord channel");
                return;
            }
        };
        let content = msg.content_safe(&ctx);
        let has_body = !content.is_empty();
        let content = format::discord_to_telegram_format(&content);
        let author = format::discord_author_name(&ctx, &msg).await;

        let mut text = format!("<b>{author}</b>\n{content}");

        let mut reply_to_message_id = None;

        if let Some(ref_msg) = msg.referenced_message.clone() {
            // if a message is being replied to, find the original message in
            // the database and reply to that message's reflection
            let found_mirror = match db::get_telegram_message_id(&self.db, ref_msg.id.into())
                .await
                .as_deref()
            {
                Ok(&[(mirror_id, _), ..]) => {
                    reply_to_message_id = Some(mirror_id);
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
        if msg
            .sticker_items
            .iter()
            .chain(
                msg.message_snapshots
                    .iter()
                    .flat_map(|s| s.sticker_items.iter()),
            )
            .any(|s| matches!(s.format_type, d::StickerFormatType::Lottie))
        {
            // since this is just a courtesy notification, ignore whether it succeeded
            let _ = msg.reply(&ctx, "Lottie format stickers (including unfortunately Discord's default stickers) are unsupported.").await;
        }

        if msg.message_snapshots.len() == 0
            || has_body
            || !msg.attachments.is_empty()
            || !msg.sticker_items.is_empty()
        {
            self.send_message(
                telegram_chat,
                msg.id,
                &text,
                msg.attachments,
                msg.sticker_items,
                reply_to_message_id,
            )
            .await;
            reply_to_message_id = None;
        }
        if msg.message_snapshots.len() > 0 {
            if msg.message_snapshots.len() > 1 {
                log::error!(
                    "More than 1 forwarded message is unsupported, {:?}",
                    &msg.message_snapshots
                )
            }
            let snapshot = msg
                .message_snapshots
                .into_iter()
                .next()
                .expect("length > 0");
            let content = d::content_safe(
                &ctx,
                snapshot.content,
                &d::ContentSafeOptions::default(),
                &[],
            );
            let content = format::discord_to_telegram_format(&content);

            let text = format!("<b>{author}</b> (forwarded)\n{content}");
            self.send_message(
                telegram_chat,
                msg.id,
                &text,
                snapshot.attachments,
                snapshot.sticker_items,
                reply_to_message_id,
            )
            .await;
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
        if upd.webhook_id.is_some_and(|id| id.is_some()) {
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
            Ok(&[(mirror_id, has_caption), ..]) => {
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
                if !has_caption {
                    let builder = self
                        .telegram_bot
                        .edit_message_text(telegram_chat, mirror_id, message_text)
                        .parse_mode(t::ParseMode::Html);

                    telegram_request!(builder.send_ref(), edbg!(author, content, text),).await;
                } else {
                    let builder = self
                        .telegram_bot
                        .edit_message_caption(telegram_chat, mirror_id)
                        .caption(message_text)
                        .parse_mode(t::ParseMode::Html);
                    telegram_request!(builder.send_ref(), edbg!(author, content, text),).await;
                }
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
        if reaction
            .user_id
            .is_some_and(|id| id == ctx.cache.current_user().id)
        {
            return;
        }
        let Some(telegram_chat) = db::get_telegram_chat_id(reaction.channel_id) else {
            log::info!("Got reaction {reaction:?} in unregistered discord channel");
            return;
        };
        let Some(emoji) = format::discord_reaction_string(&reaction.emoji) else {
            log::info!("Got reaction {reaction:?} with nameless emoji");
            return;
        };
        let discord_id = reaction.message_id;
        let telegram_id = match db::get_telegram_message_id(&self.db, discord_id)
            .await
            .as_deref()
        {
            Ok(&[(telegram_id, _), ..]) => telegram_id,
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
                reactions.entry(author).or_default().push(emoji.to_string());

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
            Err(e) => log::error!("Failed to get reaction message mapping: {}", e),
            Ok(None) => {
                // Reaction message doesn't exist, create a new one
                let author = format::discord_reactor_name(&ctx, &reaction).await;
                let text = format!("<b>Reactions</b>\n<b>{}</b>: {}", author, emoji);

                if let Some(telegram_msg) = telegram_request!(self
                    .telegram_bot
                    .send_message(telegram_chat, &text)
                    .parse_mode(t::ParseMode::Html)
                    .reply_parameters(t::ReplyParameters::new(telegram_id)))
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
        let Some(emoji) = format::discord_reaction_string(&reaction.emoji) else {
            log::info!("Got reaction {reaction:?} with nameless emoji");
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

    async fn interaction_create(&self, ctx: d::Context, interaction: d::Interaction) {
        match interaction {
            d::Interaction::Command(command) => match command.data.name.as_str() {
                "bridge" => self.handle_bridge_command(&ctx, &command).await,
                "unbridge" => self.handle_unbridge_command(&ctx, &command).await,
                "hub" => self.handle_hub_command(&ctx, &command).await,
                "unhub" => self.handle_unhub_command(&ctx, &command).await,
                "hubinfo" => {
                    let info = "Creating a Hub allows people on Telegram who know the name of the hub to bridge channels to the hub. \
                                A hub can be tied to the whole server or to a specific category.\n\
                                You can have multiple hubs in the same server and even multiple hubs tied to the same category. \
                                However, hub names must be globally unique, which means that someone could theoretically discover \
                                your hub name by attempting to create hubs until they find which fails, then bridge channels to this hub.\n\
                                However, doing so will only allow them to spam your hub with channels. It will *not* grant access \
                                to any channels other than the created ones, which is why there is no current mitigation for the issue.\n\
                                If it becomes an issue, report to the bot administrator.";
                    discord_request!(command.create_response(
                        &ctx.http,
                        d::CreateInteractionResponse::Message(
                            d::CreateInteractionResponseMessage::new().content(info),
                        ),
                    ))
                    .await;
                }
                _ => {}
            },
            d::Interaction::Autocomplete(autocomplete) => match autocomplete.data.name.as_str() {
                "bridge" => self.handle_bridge_autocomplete(&ctx, &autocomplete).await,
                "unhub" => self.handle_unhub_autocomplete(&ctx, &autocomplete).await,
                _ => {}
            },
            _ => {}
        }
    }

    // Set a handler to be called on the `ready` event. This is called when a
    // shard is booted, and a READY payload is sent by Discord. This payload
    // contains data like the current user's guild Ids, current user data,
    // private channels, and more.
    //
    // In this case, just print what the current user's username is.
    async fn ready(&self, ctx: d::Context, ready: d::Ready) {
        log::info!("{} is connected!", ready.user.name);

        // Register the bridge command
        if let Err(e) = self.register_commands(&ctx.http).await {
            log::error!("Failed to register commands: {}", e);
        }
    }
}

async fn get_telegram_attachment_as_discord(
    bot: &t::Bot,
    msg: &t::Message,
) -> Option<d::CreateAttachment> {
    let common = match &msg.kind {
        t::MessageKind::Common(common) => common,
        _ => return None,
    };
    let (file, file_name) = match common.media_kind.clone() {
        t::MediaKind::Document(t::MediaDocument { document, .. }) => {
            (document.file, document.file_name)
        }
        t::MediaKind::Photo(t::MediaPhoto { mut photo, .. }) => (photo.pop()?.file, None),
        t::MediaKind::Video(t::MediaVideo { video, .. }) => (video.file, video.file_name),
        t::MediaKind::Audio(t::MediaAudio { audio, .. }) => (audio.file, audio.file_name),
        t::MediaKind::Animation(t::MediaAnimation { animation, .. }) => {
            (animation.file, animation.file_name)
        }
        t::MediaKind::Sticker(t::MediaSticker { sticker, .. }) => (sticker.file, None),
        _ => return None,
    };

    if file.size > 50 * 1024 * 1024 {
        telegram_request!(bot
            .send_message(
                msg.chat.id,
                "File too large. Files over 50 MB won't be forwarded"
            )
            .reply_parameters(t::ReplyParameters::new(msg.id))
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
}

#[derive(Clone, Debug)]
struct AvatarCacheRecord {
    url: Arc<str>, // usually a base64 encoded image
    last_updated: Instant,
}

async fn telegram_avatar_url_by_id(
    bot: &t::Bot,
    cache: &DashMap<t::UserId, AvatarCacheRecord>,
    user_id: t::UserId,
    discord_http: &d::Http,
) -> Option<Arc<str>> {
    const CACHE_LIFETIME: Duration = Duration::from_secs(60 * 60);
    let record = cache.get(&user_id);
    let now = Instant::now();
    if let Some(record) = record {
        if record.last_updated + CACHE_LIFETIME > now {
            return Some(record.url.clone());
        }
    }
    // There was no record, or the record was invalid, we need to fetch the image

    let photos = telegram_request!(bot.get_user_profile_photos(user_id).limit(1)).await;
    let photo = photos.and_then(|photos| {
        photos
            .photos
            .get(0)
            .and_then(|sizes| sizes.iter().last().cloned())
    })?;
    let file = telegram_request!(bot.get_file(&photo.file.id)).await?;
    let t::File {
        path,
        meta: t::FileMeta { size, .. },
        ..
    } = file;
    let mut buf = Vec::with_capacity(size as usize);
    if let Err(e) = bot.download_file(&path, &mut buf).await {
        log::error!("Failed to download file: {e:?}");
        return None;
    }

    let files = vec![d::CreateAttachment::bytes(buf, "avatar.jpg")];
    let builder = d::CreateMessage::new().add_files(files.clone());
    let discord_image_channel = db::discord_image_channel().await?;
    let message = discord_request!(
        discord_image_channel.send_message(discord_http.as_ref(), builder.clone())
    )
    .await?;
    let url: Arc<str> = message
        .attachments
        .first()
        .map(|a| Arc::from(a.url.as_str()))
        .or_else(|| {
            log::error!("Failed to get avatar URL");
            None
        })?;

    let record = AvatarCacheRecord {
        url: url.clone(),
        last_updated: Instant::now(),
    };
    cache.insert(user_id, record);
    Some(url)
}

async fn discord_avatar_url_by_display_name(
    cache_http: &impl d::CacheHttp,
    channel: d::ChannelId,
    display_name: &str,
) -> Option<String> {
    let channel = discord_request!(channel.to_channel(cache_http)).await?;
    let channel = channel.guild()?;
    let members = channel.members(cache_http.cache()?).ok()?;
    let member = members
        .iter()
        .find(|m| {
            m.nick
                .as_deref()
                .or_else(|| m.user.global_name.as_deref())
                .unwrap_or(&m.user.name)
                == display_name
        })
        .or_else(|| {
            members
                .iter()
                .find(|m| m.user.global_name.as_deref().unwrap_or(&m.user.name) == display_name)
        })?;
    member.user.avatar_url()
}

struct ReplyInfo {
    content_suffix: String,
    embed: d::CreateEmbed,
    mentions: d::CreateAllowedMentions,
}

async fn reply_info(
    bot: &t::Bot,
    me: &t::Me,
    msg: &t::Message,
    avatar_cache: &Arc<DashMap<t::UserId, AvatarCacheRecord>>,
    discord_http: &d::Http,
    discord_cache: &Arc<d::Cache>,
    discord_chat: d::ChannelId,
    telegram_chat: &t::Chat,
    db: &SqlitePool,
    include_author_icon: bool,
) -> Option<ReplyInfo> {
    let cache_http = (discord_cache, discord_http);
    if let Some(ref_msg) = msg.reply_to_message() {
        let mut mentions = d::CreateAllowedMentions::new();
        let mut ref_user = None;
        let mut ref_nick = None;
        let mut ref_link = None;
        let mut ref_image = None;
        let ref_sender_telegram = ref_msg.from.as_ref().map(|f| f.id) != Some(me.id);

        match db::get_discord_message_id(&db, ref_msg.id, telegram_chat.id)
            .await
            .as_deref()
        {
            Ok(&[mirror_id, ..]) => {
                let discord_channel = discord_request!(discord_chat.to_channel(cache_http)).await?;

                let mut ref_disc_message = discord_cache
                    .message(discord_channel, mirror_id)
                    .map(|msg| msg.clone());
                if ref_disc_message.is_none() {
                    ref_disc_message =
                        discord_request!(discord_http.get_message(discord_chat, mirror_id)).await;
                }
                let mut guild_id = None;
                if let Ok(channel) = discord_chat.to_channel(cache_http).await {
                    if let Some(c) = channel.guild() {
                        guild_id = Some(c.guild_id);
                    }
                }
                ref_link = Some(mirror_id.link(discord_chat, guild_id));
                if let Some(msg) = ref_disc_message {
                    ref_nick = Some(format::discord_author_name(&cache_http, &msg).await);
                    ref_user = Some(msg.author);
                    ref_image = msg.attachments.first().map(|a| a.url.clone()).or_else(|| {
                        msg.embeds
                            .first()
                            .and_then(|e| e.image.as_ref().map(|t| t.url.clone()))
                    });
                }
            }
            Ok([]) => {
                log::info!("Replying to message {ref_msg:?} with no known counterpart");
            }
            Err(e) => {
                log::error!("Database lookup failed: {e}");
            }
        }
        let ref_text = msg
            .quote()
            .map(|q| {
                (
                    q.text.as_str(),
                    t::MessageEntityRef::parse(&q.text, &q.entities),
                )
            })
            .or_else(|| ref_msg.text().zip(ref_msg.parse_entities()))
            .or_else(|| ref_msg.caption().zip(ref_msg.parse_caption_entities()))
            .map(|(t, e)| format::telegram_to_discord_format(t, e))
            .unwrap_or_default();
        let (ref_author, ref_text) = if ref_sender_telegram {
            let ref_author = format::telegram_author_name(ref_msg);
            (ref_author, ref_text)
        } else {
            let mut lines = ref_text.lines();
            let first_line = lines.next().unwrap_or_else(|| {
                log::error!("message from bot has no lines");
                ""
            });
            let ref_text = lines.collect::<Vec<_>>().join("\n> ");
            let ref_author = if let Some(author) = &ref_user {
                let mention = author.id.mention();
                mentions = mentions.users(Some(author.id));
                mention.to_string()
            } else {
                first_line
                    .split("**")
                    .nth(1)
                    .unwrap_or("Unknown")
                    .to_string()
            };
            (ref_author, ref_text)
        };
        let reply_str = if msg.quote().is_some() {
            "quoting"
        } else {
            "replying to"
        };
        let reply_str = if let Some(link) = ref_link {
            format!("[{reply_str}]({link})")
        } else {
            reply_str.to_string()
        };
        let mut embed_author = d::CreateEmbedAuthor::new(ref_nick.as_ref().unwrap_or(&ref_author));
        if include_author_icon {
            if ref_sender_telegram {
                if let Some(from) = &ref_msg.from {
                    if let Some(url) =
                        telegram_avatar_url_by_id(bot, &*avatar_cache, from.id, discord_http).await
                    {
                        embed_author = embed_author.icon_url(&*url);
                    }
                }
            } else if let Some(url) = ref_user.as_ref().and_then(|u| u.avatar_url()) {
                embed_author = embed_author.icon_url(&*url);
            }
        }
        fn preview(s: &str) -> String {
            const MAX_LENGTH: usize = 200;
            const MAX_LINES: usize = 5;
            let mut changed = false;
            let s = if s.chars().count() > MAX_LENGTH {
                changed = true;
                &s[..s
                    .char_indices()
                    .take(MAX_LENGTH)
                    .filter(|&(_, c)| c.is_whitespace())
                    .last()
                    .unwrap()
                    .0]
            } else {
                s
            };
            let s = if let Some(last_linebreak) = s
                .char_indices()
                .filter(|&(_, c)| c == '\n')
                .nth(MAX_LINES - 1)
            {
                changed = true;
                &s[..last_linebreak.0 + 1]
            } else {
                s
            };
            let mut s = s.to_string();
            if changed {
                s.push_str("...");
            }
            s
        }
        let ref_text = preview(&ref_text);
        let mut embed = d::CreateEmbed::new()
            .description(ref_text)
            .author(embed_author);
        if let Some(image) = ref_image {
            embed = embed.thumbnail(image);
        }
        Some(ReplyInfo {
            content_suffix: format!("-# **{reply_str} {ref_author}**"),
            embed,
            mentions,
        })
    } else {
        None
    }
}

fn unicode_keycap(digit: usize) -> &'static str {
    [
        "\u{0030}\u{fe0f}\u{20e3}",
        "\u{0031}\u{fe0f}\u{20e3}",
        "\u{0032}\u{fe0f}\u{20e3}",
        "\u{0033}\u{fe0f}\u{20e3}",
        "\u{0034}\u{fe0f}\u{20e3}",
        "\u{0035}\u{fe0f}\u{20e3}",
        "\u{0036}\u{fe0f}\u{20e3}",
        "\u{0037}\u{fe0f}\u{20e3}",
        "\u{0038}\u{fe0f}\u{20e3}",
        "\u{0039}\u{fe0f}\u{20e3}",
        // "\u{1f51f}",
    ][digit]
}

async fn send_poll(
    webhook: d::Webhook,
    avatar_handle: tokio::task::JoinHandle<Option<Arc<str>>>,
    discord_http: Arc<d::Http>,
    poll: &t::Poll,
    author: &str,
) -> Option<d::Message> {
    let embed = d::CreateEmbed::new()
        .title(match (&poll.poll_type, poll.allows_multiple_answers) {
            (t::PollType::Quiz, _) => "Quiz",
            (_, false) => "Poll (pick one)",
            (_, true) => "Poll (multiple)",
        })
        .description(&poll.question)
        .fields(
            poll.options
                .iter()
                .enumerate()
                .map(|(i, opt)| (format!("Option {}", i), &opt.text, false)),
        );
    let mut builder = d::ExecuteWebhook::new().username(author).embed(embed);
    if let Ok(Some(avatar_url)) = avatar_handle.await {
        builder = builder.avatar_url(&*avatar_url);
    }
    let discord_result = discord_request!(
        webhook.execute(discord_http.clone(), true, builder.clone()),
        edbg!("Poll")
    )
    .await
    .flatten();
    if let Some(msg) = &discord_result {
        for i in 0..poll.options.len() {
            _ = msg
                .react(
                    &discord_http,
                    d::ReactionType::Unicode(unicode_keycap(i).to_string()),
                )
                .await;
        }
    }
    discord_result
}

async fn handle_telegram_bridge_command(bot: t::Bot, http: Arc<d::Http>, msg: &t::Message) {
    let Some(target) = msg
        .text()
        .and_then(|s| s.strip_prefix("/bridge "))
        .map(str::trim)
    else {
        return;
    };
    macro_rules! reply {
        ($err:expr $(,)?) => {{
            let err = bot
                .send_message(msg.chat.id, $err)
                .parse_mode(t::ParseMode::Html)
                .reply_parameters(t::ReplyParameters::new(msg.id));
            let _ = telegram_request!(err.send_ref()).await;
        }};
    }
    let Some(from) = &msg.from else { return };
    let Some(from) = telegram_request!(bot.get_chat_member(msg.chat.id, from.id)).await else {
        return;
    };
    if !from.can_manage_chat() {
        reply!("Only administrators capable of managing the chat can create bridges.")
    }
    if let Some((prev_discord_id, _)) = db::get_discord_channel_id(msg.chat.id) {
        reply!(format!(
            "This chat is already linked to the discord chat with ID: {prev_discord_id}. Remove with /unbridge first."
        ));
        return;
    }
    if target.contains(char::is_whitespace) || target == "" {
        reply!(
            "Usage: <code>/bridge &lthub name&gt</code> \
            where <code>&lthub name&gt</code> contains no whitespace"
        );
        return;
    }
    let Some(hub) = db::get_hub_server(target).await else {
        reply!(format!("No hub found matching \"{target}\""));
        return;
    };
    let chat_name = msg
        .chat
        .title()
        .or_else(|| msg.chat.username())
        .unwrap_or("unknown chat name");
    let create_channel = d::CreateChannel::new(chat_name).kind(d::ChannelType::Text);
    let channel = match hub {
        db::Hub::Server(g) => {
            discord_request!(g.create_channel(&http, create_channel.clone())).await
        }
        db::Hub::Category(g, c) => {
            discord_request!(g.create_channel(&http, create_channel.clone().category(c))).await
        }
    };
    let Some(ch) = channel else {
        reply!(
            "Could not create channel. \
            Ensure that bot has required Discord permissions in the hub \
            and hub category exists or try again later."
        );
        return;
    };
    if let Err(e) = db::set_chat_mapping(ch.id, msg.chat.id, None).await {
        log::error!(
            "Failed to set mapping for created channel: {e}. Attempting to delete channel."
        );
        if let Some(_) = discord_request!(ch.delete(&http)).await {
            log::warn!("Successfully deleted created channel.");
            reply!("An internal error occurred. Try again later.");
            return;
        } else {
            log::error!("Could not delete created channel.");
            let explanation = d::CreateMessage::new().content(
                "[Hub]: Someone attempted to bridge to this hub, \
                but an internal error occurred and then this channel could not be deleted. \
                This channel can be safely deleted.",
            );
            let _ = discord_request!(ch.send_message(&http, explanation.clone())).await;
            reply!("The Discord channel was created but a bridge could not be made due to an internal error.");
            return;
        }
    }
    reply!("Successfully created and linked channel.");
    let explanation = d::CreateMessage::new().content(format!(
        "[Hub]: Someone bridged the telegram channel \"{chat_name}\" to this hub. \
        If this appears to be from someone you do not know, \
        you should delete the hub and create a new one with a different name. \
        Use `/hubinfo` for more info."
    ));
    let _ = discord_request!(ch.send_message(&http, explanation.clone())).await;
}

async fn handle_telegram_unbridge_command(bot: t::Bot, http: Arc<d::Http>, msg: &t::Message) {
    macro_rules! reply {
        ($err:expr $(,)?) => {{
            let err = bot
                .send_message(msg.chat.id, $err)
                .parse_mode(t::ParseMode::Html)
                .reply_parameters(t::ReplyParameters::new(msg.id));
            let _ = telegram_request!(err.send_ref()).await;
        }};
    }
    let Some(from) = &msg.from else { return };
    let Some(from) = telegram_request!(bot.get_chat_member(msg.chat.id, from.id)).await else {
        return;
    };
    if !from.can_manage_chat() {
        reply!("Only administrators capable of managing the chat can create bridges.")
    }
    let Some((prev_discord_id, _)) = db::get_discord_channel_id(msg.chat.id) else {
        reply!("This chat is not bridged to any chats.");
        return;
    };
    if let Err(e) = db::remove_chat_mapping(db::RemovalChatId::Telegram(msg.chat.id)).await {
        log::error!("Failed to remove chapping: {e}");
        reply!("An internal error occurred trying to remove bridge.");
        return;
    }
    reply!("Successfully removed bridge!");
    let notification =
        d::CreateMessage::new().content("The bridge to this channel has been removed.");
    let _ = discord_request!(prev_discord_id.send_message(&http, notification.clone()));
}

async fn handle_update(
    bot: t::Bot,
    me: t::Me,
    avatar_cache: Arc<DashMap<t::UserId, AvatarCacheRecord>>,
    upd: t::Update,
    discord_http: Arc<d::Http>,
    discord_cache: Arc<d::Cache>,
    webhook_cache: Arc<DashMap<d::ChannelId, d::Webhook>>,
    db: SqlitePool,
) -> Result<(), eyre::Report> {
    log::info!("{upd:?}");

    let cache_http = (&discord_cache, discord_http.as_ref());

    let Some(telegram_chat) = upd.chat().cloned() else {
        log::error!("Got update {upd:?} without a chat");
        return Ok(());
    };
    let is_member = !matches!(
        upd.kind,
        t::UpdateKind::MyChatMember(t::ChatMemberUpdated {
            new_chat_member: t::ChatMember {
                kind: t::ChatMemberKind::Banned(_) | t::ChatMemberKind::Left,
                ..
            },
            ..
        })
    );

    let title = telegram_chat
        .title()
        .or_else(|| telegram_chat.username())
        .unwrap_or("Unnamed chat")
        .to_string();
    if let Err(e) = db::update_chat_membership(&db, telegram_chat.id, &title, is_member).await {
        log::error!("Failed to update chat membership: {e:?}");
    }
    if let t::UpdateKind::Message(msg) = &upd.kind {
        if let Some(text) = msg.text() {
            if text.starts_with("/bridge") {
                handle_telegram_bridge_command(bot, discord_http, msg).await;
                return Ok(());
            } else if text == "/unbridge" {
                handle_telegram_unbridge_command(bot, discord_http, msg).await;
                return Ok(());
            }
        }
    }
    let Some((discord_chat, webhook_url)) = db::get_discord_channel_id(telegram_chat.id) else {
        log::info!("Got message {upd:?} in unregistered telegram chat");
        return Ok(());
    };

    let cached_webhook = webhook_cache.get(&discord_chat).map(|w| w.clone());

    let webhook = match (cached_webhook, webhook_url) {
        (Some(webhook), _) => webhook,
        (None, Some(url)) => {
            let webhook =
                discord_request!(d::Webhook::from_url(discord_http.clone(), url.as_str())).await;
            match webhook {
                Some(webhook) => {
                    webhook_cache.insert(discord_chat, webhook.clone());
                    webhook
                }
                None => {
                    log::error!("Failed to get webhook");
                    return Ok(());
                }
            }
        }
        (None, None) => {
            let make_webhook = d::CreateWebhook::new("Discogram");
            let webhook = discord_request!(
                discord_chat.create_webhook(discord_http.clone(), make_webhook.clone())
            )
            .await;
            match webhook {
                Some(webhook) => {
                    if let Err(e) = db::set_chat_mapping(
                        discord_chat,
                        telegram_chat.id,
                        Some(webhook.url().expect("Bot running without token")),
                    )
                    .await
                    {
                        log::error!("Failed to insert chat mapping: {}", e);
                    }
                    webhook_cache.insert(discord_chat, webhook.clone());
                    webhook
                }
                None => {
                    log::error!("Failed to create webhook");
                    return Ok(());
                }
            }
        }
    };

    match upd.kind {
        t::UpdateKind::Message(msg) => {
            if let Some(msg) = msg.pinned_message() {
                match db::get_discord_message_id(&db, msg.id(), msg.chat().id)
                    .await
                    .as_deref()
                {
                    Ok(&[discord_id, ..]) => {
                        let _ = discord_request!(discord_http.pin_message(
                            discord_chat,
                            discord_id,
                            Some("Telegram pin")
                        ))
                        .await;
                    }
                    Ok([]) => {
                        log::info!("TODO: implement handling of pins of unmapped messages I guess")
                    }
                    Err(e) => log::error!("Failed to get message mapping: {}", e),
                }
                return Ok(());
            }
            let author = format::telegram_author_name(&msg);
            let text = msg
                .text()
                .zip(msg.parse_entities())
                .or_else(|| msg.caption().zip(msg.parse_caption_entities()))
                .map(|(t, e)| format::telegram_to_discord_format(t, e));
            // let mut content = format!("**{author}**\n{}", text.as_deref().unwrap_or(""));
            let mut content = text.unwrap_or_default();
            let avatar_handle = {
                let bot = bot.clone();
                let avatar_cache = avatar_cache.clone();
                let from = msg.from.clone();
                let discord_http = discord_http.clone();
                tokio::spawn(async move {
                    match from {
                        Some(u) => {
                            telegram_avatar_url_by_id(&bot, &avatar_cache, u.id, &*discord_http)
                                .await
                        }
                        None => None,
                    }
                })
            };
            if let Some(poll) = msg.poll() {
                if let Some(discord_msg) =
                    send_poll(webhook, avatar_handle, discord_http, poll, &author).await
                {
                    if let Err(e) =
                        db::insert_mapping(&db, discord_msg.id, msg.id, telegram_chat.id, false)
                            .await
                    {
                        log::error!("Failed to insert message mapping: {}", e);
                    }
                }
                return Ok(());
            }
            let mut message = d::ExecuteWebhook::new().username(&author);
            let mut embed = None;

            if let Some(origin) = msg.forward_origin() {
                let mut embed_content = &*content;
                let original_author = match origin {
                    t::MessageOrigin::User { sender_user, .. } => sender_user.full_name(),
                    t::MessageOrigin::Chat {
                        sender_chat: chat, ..
                    }
                    | t::MessageOrigin::Channel { chat, .. } => chat
                        .title()
                        .or_else(|| chat.username())
                        .unwrap_or("Unknown")
                        .to_string(),
                    t::MessageOrigin::HiddenUser {
                        sender_user_name, ..
                    } => sender_user_name.clone(),
                };
                let mut original_author = d::CreateEmbedAuthor::new(&original_author);
                'set_author: {
                    if let t::MessageOrigin::User { sender_user, .. } = origin {
                        if sender_user.id == me.id && !content.starts_with("**Reactions**\n") {
                            let name = content
                                .lines()
                                .next()
                                .and_then(|s| s.strip_prefix("**"))
                                .and_then(|s| s.strip_suffix("**"))
                                .unwrap_or("Unknown [this shouldn't be possible]");
                            original_author = original_author.name(name);
                            embed_content = embed_content
                                .split_once('\n')
                                .map_or(embed_content, |(_, rest)| rest);
                            if let Some(url) =
                                discord_avatar_url_by_display_name(&cache_http, discord_chat, name)
                                    .await
                            {
                                original_author = original_author.icon_url(&*url);
                                break 'set_author;
                            }
                        }
                        if let Some(url) = telegram_avatar_url_by_id(
                            &bot,
                            &avatar_cache,
                            sender_user.id,
                            &*discord_http,
                        )
                        .await
                        {
                            original_author = original_author.icon_url(&*url);
                        }
                    }
                }
                embed = Some(
                    d::CreateEmbed::new()
                        .author(original_author)
                        .description(embed_content),
                );
                content = "-# Forwarded message".to_string();
            }

            if let Some(ReplyInfo {
                content_suffix,
                embed,
                mentions,
            }) = reply_info(
                &bot,
                &me,
                &msg,
                &avatar_cache,
                &discord_http,
                &discord_cache,
                discord_chat,
                &telegram_chat,
                &db,
                true,
            )
            .await
            {
                message = message.embed(embed).allowed_mentions(mentions);
                content = format!("{content}\n{content_suffix}");
            }

            if let Some(attachment) = get_telegram_attachment_as_discord(&bot, &msg).await {
                if let Some(e) = embed {
                    embed = Some(e.attachment(&attachment.filename));
                }
                message = message.add_file(attachment);
            }

            if let Some(e) = embed {
                message = message.embed(e);
            }

            if let Ok(Some(avatar_url)) = avatar_handle.await {
                message = message.avatar_url(&*avatar_url);
            }

            message = message.content(&content);

            let discord_result = discord_request!(
                webhook.execute(discord_http.clone(), true, message.clone()),
                edbg!(author, msg.text().unwrap_or(""), content)
            )
            .await;

            if let Some(Some(discord_msg)) = discord_result {
                if let Err(e) =
                    db::insert_mapping(&db, discord_msg.id, msg.id, telegram_chat.id, false).await
                {
                    log::error!("Failed to insert message mapping: {}", e);
                }
            }
        }
        t::UpdateKind::EditedMessage(msg) => {
            match db::get_discord_message_id(&db, msg.id, telegram_chat.id)
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
                        let discord_result = discord_request!(webhook.delete_message(
                            discord_http.clone(),
                            None,
                            mirror_id
                        ))
                        .await;
                        if let Some(()) = discord_result {
                            if let Err(e) =
                                db::delete_by_telegram(&db, msg.id, telegram_chat.id).await
                            {
                                log::error!("Failed to delete message mapping: {}", e);
                            }
                        }
                        return Ok(());
                    }
                    let author = format::telegram_author_name(&msg);
                    let mut content = text.unwrap_or_default();
                    let mut mentions = d::CreateAllowedMentions::new();

                    if let Some(ReplyInfo {
                        content_suffix,
                        mentions: new_mentions,
                        ..
                    }) = reply_info(
                        &bot,
                        &me,
                        &msg,
                        &avatar_cache,
                        &discord_http,
                        &discord_cache,
                        discord_chat,
                        &telegram_chat,
                        &db,
                        false,
                    )
                    .await
                    {
                        mentions = new_mentions;
                        content = format!("{content}\n{content_suffix}");
                    }

                    let message = d::EditWebhookMessage::new()
                        .content(&content)
                        .allowed_mentions(mentions);

                    discord_request!(
                        webhook.edit_message(cache_http, mirror_id, message.clone()),
                        edbg!(author, msg.text().unwrap_or(""), content)
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
        t::UpdateKind::MessageReaction(reaction) => {
            let telegram_id = reaction.message_id;
            let discord_id = match db::get_discord_message_id(&db, telegram_id, telegram_chat.id)
                .await
                .as_deref()
            {
                Ok(&[discord_id, ..]) => discord_id,
                Ok([]) => {
                    log::info!("Got reaction for unknown message, {reaction:?}");
                    return Ok(());
                }
                Err(e) => {
                    log::error!("Failed to get reaction message mapping: {}", e);
                    return Ok(());
                }
            };
            match db::get_discord_reaction_message_id(&db, telegram_id, telegram_chat.id).await {
                Ok(Some((reaction_message_id, reactions))) => {
                    let mut reactions = format::parse_discord_reaction_message(&reactions);
                    let author = format::telegram_reactor_name(&reaction);
                    let _ = reactions.insert(
                        author,
                        format::filter_telegram_reactions(&reaction.new_reaction),
                    );

                    let new_text = format::format_discord_reaction_message(&reactions);
                    if new_text == "**Reactions**" {
                        if let Some(_) = discord_request!(
                            discord_chat.delete_message(&*discord_http, reaction_message_id)
                        )
                        .await
                        {
                            if let Err(e) = db::remove_reaction_mapping_by_telegram(
                                &db,
                                telegram_id,
                                telegram_chat.id,
                            )
                            .await
                            {
                                log::error!("Failed to remove reaction message mapping: {}", e);
                            }
                        }
                    } else {
                        if let Some(_) = discord_request!(discord_chat.edit_message(
                            &*discord_http,
                            reaction_message_id,
                            d::EditMessage::new().content(&new_text)
                        ))
                        .await
                        {
                            if let Err(e) = db::update_discord_reaction_mapping(
                                &db,
                                telegram_id,
                                telegram_chat.id,
                                &new_text,
                            )
                            .await
                            {
                                log::error!("Failed to update reaction message mapping: {}", e);
                            }
                        }
                    }
                }
                Ok(None)
                    if reaction
                        .new_reaction
                        .iter()
                        .any(|r| matches!(r, t::ReactionType::Emoji { .. })) =>
                {
                    let author = format::telegram_reactor_name(&reaction);
                    let emojis =
                        format::filter_telegram_reactions(&reaction.new_reaction).join(", ");
                    let content = format!("**Reactions**\n**{author}**: {emojis}");
                    let reaction_msg = d::CreateMessage::new()
                        .content(&content)
                        .reference_message((discord_chat, discord_id))
                        .allowed_mentions(d::CreateAllowedMentions::new().replied_user(false));

                    if let Some(reaction_msg) = discord_request!(
                        discord_chat.send_message(&*discord_http, reaction_msg.clone()),
                        edbg!(author, emojis, content)
                    )
                    .await
                    {
                        if let Err(e) = db::insert_reaction_mapping(
                            &db,
                            reaction_msg.id,
                            telegram_id,
                            telegram_chat.id,
                            &content,
                        )
                        .await
                        {
                            log::error!("Failed to insert reaction mapping: {}", e);
                        }
                    }
                }
                Ok(None) => log::info!("Got reaction removal for unknown message, {reaction:?}"),
                Err(e) => log::error!("Failed to get reaction message mapping: {}", e),
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
            // this specific variant of HttpError is not a retryable error
            Err(why @ d::Error::Http(d::HttpError::UnsuccessfulRequest(_))) => {
                log::error!("Failed discord request: {why:?}");
                log();
                break None;
            }
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
    let intents = d::GatewayIntents::all();

    // Create a new instance of the Client, logging in as a bot. This will
    // automatically prepend your bot token with "Bot ", which is a requirement
    // by Discord for bot users.
    let mut discord_client = d::Client::builder(&discord_token, intents)
        .event_handler(DiscordState {
            telegram_bot: telegram_bot.clone(),
            db: db_pool.clone(),
        })
        .await
        .expect("Err creating client");

    let discord_cache = discord_client.cache.clone();
    let discord_http = discord_client.http.clone();

    log::info!("Starting telegram...");

    let webhook_cache = Arc::new(DashMap::<d::ChannelId, d::Webhook>::new());
    let avatar_cache = Arc::new(DashMap::<t::UserId, AvatarCacheRecord>::new());

    let telegram_handler = t::dptree::endpoint(handle_update);

    let mut telegram_dispatch = t::Dispatcher::builder(telegram_bot, telegram_handler)
        .dependencies(t::dptree::deps![
            webhook_cache,
            avatar_cache,
            discord_cache,
            discord_http,
            db_pool
        ])
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
