use eyre::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct MessageId(pub u64);

impl From<u64> for MessageId {
    fn from(id: u64) -> Self {
        MessageId(id)
    }
}

impl From<MessageId> for u64 {
    fn from(id: MessageId) -> Self {
        id.0
    }
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum FileData {
    Url(Url),
    Blob(Arc<[u8]>),
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum FileKind {
    Image,
    Video,
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
    Message(MessageId),
    Unknown,
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct Message {
    pub author: Author,
    pub content: RichText,
    pub attachments: Vec<File>,
    pub reply_to: Option<MessageId>,
    pub forwarded_from: Option<ForwardInfo>,
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct Reaction {
    pub message: MessageId,
    pub author: Author,
    pub content: String,
}

pub enum Event {
    Message(Message),
    MessageDelete(MessageId),
    MessageEdit { from: Option<Message>, to: Message },
    ReactionAdd(Reaction),
    ReactionRemove(Reaction),
}

pub trait Portal {
    async fn message(&self, id: MessageId, msg: Message) -> Result<()>;
    async fn message_delete(&self, id: MessageId) -> Result<()>;
    async fn message_edit(&self, id: MessageId, from: Option<Message>, to: Message) -> Result<()>;
    async fn reaction_add(&self, reaction: Reaction) -> Result<()>;
    async fn reaction_remove(&self, reaction: Reaction) -> Result<()>;
}
