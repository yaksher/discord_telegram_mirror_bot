use crate::db;
use eyre::{bail, eyre, Result};
use serde::{Deserialize, Serialize};
use serenity::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;

use url::Url;

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum RichText {
    Sequence(Vec<RichText>),
    Bold(Box<RichText>),
    Italic(Box<RichText>),
    Strikethrough(Box<RichText>),
    Blockquote(Box<RichText>),
    FixedWidth(String),
    Hyperlink {
        text: String,
        link: Url,
    },
    Code {
        language: Option<String>,
        body: String,
    },
    Plain(String),
}

macro_rules! impl_id {
    ($($t:ident),*$(,)?) => {
        $(#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
        pub struct $t(pub u64);

        impl From<u64> for $t {
            fn from(id: u64) -> Self {
                Self(id)
            }
        }

        impl From<$t> for u64 {
            fn from(id: $t) -> Self {
                id.0
            }
        })*
    };
}

impl_id! {
    MessageId,
    AuthorId,
}

pub type ExternAuthorId = Arc<str>;
pub type ExternMessageId = Arc<str>;

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum FileData {
    Url(Url),
    Blob(Arc<[u8]>),
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum FileKind {
    Image,
    Video,
    Audio,
    Document,
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct File {
    pub name: String,
    pub data: FileData,
    pub kind: FileKind,
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct Author {
    pub username: String,
    pub display_name: Option<String>,
    pub pfp: Option<Url>,
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum ForwardInfo {
    Author(Author),
    Name(String),
    Unknown,
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct MessageData {
    pub author: Author,
    pub content: RichText,
    pub attachments: Vec<File>,
    pub forwarded_from: Option<ForwardInfo>,
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct MessageMeta {
    pub reply_to: Option<(ExternMessageId, MessageData)>,
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct Message(pub MessageMeta, pub MessageData);

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct Reaction {
    pub author: Author,
    pub content: String,
}

const EVENTS_CHANNEL_SIZE: usize = 16; // arbitrary

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum EventKind {
    Message(Message),
    MessageDelete,
    MessageEdit { from: Option<Message>, to: Message },
    ReactionAdd(Reaction),
    ReactionRemove(Reaction),
}

pub struct Event {
    author_id: ExternAuthorId,
    msg_id: ExternMessageId,
    kind: EventKind,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct PortalId(u64);

impl PortalId {
    pub fn raw(&self) -> u64 {
        self.0
    }
}

#[async_trait]
pub trait Portal {
    fn id(&self) -> PortalId;
    async fn start(&self, id: PortalId, events: mpsc::Sender<(PortalId, Event)>);
    async fn message(&self, msg: Message) -> Result<Vec<ExternMessageId>>;
    async fn message_delete(&self, id: ExternMessageId) -> Result<()>;
    async fn message_edit(
        &self,
        id: ExternMessageId,
        from: Option<Message>,
        to: Message,
    ) -> Result<()>;
    async fn reaction_add(&self, id: ExternMessageId, reaction: Reaction) -> Result<()>;
    async fn reaction_remove(&self, id: ExternMessageId, reaction: Reaction) -> Result<()>;
}

async fn handle_event<P>(
    persist: &db::Persist,
    portals: &[P],
    source: PortalId,
    event: Event,
) -> Result<()>
where
    P: std::ops::Deref<Target = dyn Portal>,
{
    let Event {
        author_id: _src_ath_id,
        msg_id: src_msg_id,
        kind,
    } = event;

    use EventKind as EK;
    let result = if let EK::Message(msg) = kind.clone() {
        let internal_msg_id = persist.insert_message(&msg).await?;
        persist
            .add_message_mapping(internal_msg_id, source, src_msg_id)
            .await?;
        let Message(meta, data) = msg;
        let reply_to_info = match &meta.reply_to {
            Some((id, data)) => Some((persist.get_message_id(id.clone(), source).await?, data)),
            None => None,
        };
        futures::future::join_all(portals.iter().filter(|p| p.id() != source).map(|p| async {
            let reply_to = match reply_to_info {
                Some((id, data)) => {
                    let Some(reply_msg_id) = persist
                        .get_messages_for_portal(id.clone(), source)
                        .await?
                        .get(0)
                        .cloned()
                    else {
                        bail!("No messages found for {id:?}, {:?}", p.id());
                    };
                    Some((reply_msg_id, data.clone()))
                }
                None => None,
            };
            let meta = MessageMeta { reply_to };
            let msg = Message(meta, data.clone());
            let result = p.message(msg).await;
            match result {
                Ok(extern_ids) => {
                    for id in extern_ids {
                        persist
                            .add_message_mapping(internal_msg_id, p.id(), id)
                            .await?;
                    }
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }))
        .await
    } else {
        let internal_msg_id = persist.get_message_id(src_msg_id.clone(), source).await?;
        futures::future::join_all(portals.iter().filter(|p| p.id() != source).map(|p| async {
            let ext_msg_ids = persist
                .get_messages_for_portal(internal_msg_id, p.id())
                .await?;
            let ext_msg_id = ext_msg_ids.get(0).cloned().ok_or(eyre!(
                "No messages found for {internal_msg_id:?}, {:?}",
                p.id()
            ))?;
            match kind.clone() {
                EK::MessageDelete => {
                    // TODO: delete message from database
                    for ext_msg_id in ext_msg_ids {
                        p.message_delete(ext_msg_id).await?;
                    }
                    Ok(())
                }
                EK::MessageEdit { from, to } => p.message_edit(ext_msg_id, from, to).await,
                // TODO: update reaction mappings rather than making the portal do it
                EK::ReactionAdd(reaction) => p.reaction_add(ext_msg_id, reaction).await,
                // TODO: update reaction mappings rather than making the portal do it
                EK::ReactionRemove(reaction) => p.reaction_remove(ext_msg_id, reaction).await,
                EK::Message(_) => unreachable!(),
            }
        }))
        .await
    };
    let errs = result
        .into_iter()
        .enumerate()
        .filter_map(|(i, r)| r.err().map(|e| (i, e)))
        .map(|(i, e)| format!("Portal {i}: {e}"))
        .collect::<Vec<_>>();
    if !errs.is_empty() {
        bail!("Errors for portals: {}", errs.join("\n"));
    }
    Ok(())
}
pub async fn run<P>(persist: db::Persist, portals: &[P])
where
    P: std::ops::Deref<Target = dyn Portal>,
{
    let (events_in, mut events) = mpsc::channel(EVENTS_CHANNEL_SIZE);
    for (i, portal) in portals.iter().enumerate() {
        portal.start(PortalId(i as u64), events_in.clone()).await
    }
    while let Some((source, event)) = events.recv().await {
        match handle_event(&persist, portals, source, event).await {
            Ok(()) => (),
            Err(e) => {
                eprintln!("Error handling event: {e:?}");
            }
        }
    }
}
