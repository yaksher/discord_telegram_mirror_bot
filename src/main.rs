#![allow(dead_code)]
mod db;
mod db_old;
mod format;
mod model;
mod old;

use dashmap::DashMap;
use sqlx::SqlitePool;
use tokio::time::Instant;

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
        builder::{
            CreateAllowedMentions, CreateAttachment, CreateEmbed, CreateEmbedAuthor, CreateMessage,
            CreateWebhook, EditMessage, EditWebhookMessage, ExecuteWebhook,
        },
        cache::Cache,
        http::Http,
        model::{
            channel::{Message, Reaction, ReactionType},
            event::MessageUpdateEvent,
            gateway::Ready,
            id::{ChannelId, GuildId, MessageId},
            webhook::{Webhook, WebhookChannel, WebhookGuild, WebhookType},
        },
        prelude::*,
        Error,
    };
}
use serenity::prelude::*;

use discord as d;
use telegram as t;

const DISCORD_TOKEN_ENV: &str = "DISCORD_TOKEN";
const DISCORD_IMAGE_CHANNEL: d::ChannelId = d::ChannelId::new(1267352463158153216);

fn main() {}
