use eyre::Result;
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
    Fixed(String),
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

pub type ExternAuthorId = String;
pub type ExternMessageId = String;

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
pub struct ExternMessageMeta {
    pub reply_to: Option<(ExternMessageId, MessageData)>,
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct MessageMeta {
    pub reply_to: Option<MessageId>,
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct Message(MessageMeta, MessageData);

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct Reaction {
    pub author: Author,
    pub content: String,
}

const EVENTS_CHANNEL_SIZE: usize = 16; // arbitrary

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum EventKind {
    Message(ExternMessageMeta, MessageData),
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

#[async_trait]
pub trait Portal {
    fn id(&self) -> PortalId;
    async fn start(&self, id: PortalId, events: mpsc::Sender<(PortalId, Event)>);
    async fn message(
        &self,
        ath_id: AuthorId,
        id: MessageId,
        msg: Message,
    ) -> Result<Vec<ExternMessageId>>;
    async fn message_delete(&self, ath_id: AuthorId, id: MessageId) -> Result<()>;
    async fn message_edit(
        &self,
        ath_id: AuthorId,
        id: MessageId,
        from: Option<Message>,
        to: Message,
    ) -> Result<()>;
    async fn reaction_add(&self, ath_id: AuthorId, id: MessageId, reaction: Reaction)
        -> Result<()>;
    async fn reaction_remove(
        &self,
        ath_id: AuthorId,
        id: MessageId,
        reaction: Reaction,
    ) -> Result<()>;
}

#[allow(unused_variables, unreachable_code)]
pub async fn run(name: String, portals: Vec<Box<dyn Portal>>) {
    let (events_in, mut events) = mpsc::channel(EVENTS_CHANNEL_SIZE);
    for (i, portal) in portals.iter().enumerate() {
        portal.start(PortalId(i as u64), events_in.clone()).await
    }
    while let Some((
        sender,
        Event {
            author_id,
            msg_id,
            kind,
        },
    )) = events.recv().await
    {
        use EventKind as EK;
        let ath_id = todo!();
        let msg_id = todo!();
        futures::future::join_all(portals.iter().filter(|p| p.id() != sender).map(
            |portal| async {
                match kind.clone() {
                    EK::Message(meta, data) => {
                        let meta = todo!(); // canonicalize meta via db
                        let msg = Message(meta, data);
                        let result = portal.message(ath_id, msg_id, msg).await;
                        match result {
                            Ok(extern_ids) => {
                                todo!(
                                    "Handle external message IDs: {:?}",
                                    extern_ids
                                        .into_iter()
                                        .map(|x| x.to_string())
                                        .collect::<Vec<_>>()
                                );
                            }
                            Err(e) => Err(e),
                        }
                    }
                    EK::MessageDelete => portal.message_delete(ath_id, msg_id).await,
                    EK::MessageEdit { from, to } => {
                        portal.message_edit(ath_id, msg_id, from, to).await
                    }
                    EK::ReactionAdd(reaction) => {
                        portal.reaction_add(ath_id, msg_id, reaction).await
                    }
                    EK::ReactionRemove(reaction) => {
                        portal.reaction_remove(ath_id, msg_id, reaction).await
                    }
                }
            },
        ))
        .await;
    }
}
