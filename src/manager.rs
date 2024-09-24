use grammers_client::session::Session;
use grammers_client::InputMessage;
use grammers_client::{
    types::{Chat, Update},
    Client, Config, SignInError,
};
use std::env;
use std::io::{self, BufRead as _, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, OnceCell};

use crate::discord as d;

type Error = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, Error>;

const SESSION_FILE: &str = "manager.session";
const API_ID_ENV: &str = "MANAGER_API_ID";
const API_HASH_ENV: &str = "MANAGER_API_HASH";
fn prompt(message: &str) -> Result<String> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    stdout.write_all(message.as_bytes())?;
    stdout.flush()?;

    let stdin = io::stdin();
    let mut stdin = stdin.lock();

    let mut line = String::new();
    stdin.read_line(&mut line)?;
    Ok(line)
}

async fn connect_client() -> Result<Client> {
    let api_id = env::var(API_ID_ENV)
        .map_err(|e| Box::new(e) as Error)
        .and_then(|v| v.parse::<i32>().map_err(|e| Box::new(e) as Error))
        .inspect_err(|_| DO_NOT_RETRY.store(true, Ordering::Relaxed))?;
    let api_hash =
        env::var(API_HASH_ENV).inspect_err(|_| DO_NOT_RETRY.store(true, Ordering::Relaxed))?;

    let client = Client::connect(Config {
        session: Session::load_file_or_create(SESSION_FILE)?,
        api_id,
        api_hash: api_hash.clone(),
        params: Default::default(),
    })
    .await?;

    let mut sign_out = false;

    if !client.is_authorized().await? {
        let phone = prompt("Enter your phone number (international format): ")?;
        let token = client.request_login_code(&phone).await?;
        let code = prompt("Enter the code you received: ")?;
        let signed_in = client.sign_in(&token, &code).await;
        match signed_in {
            Err(SignInError::PasswordRequired(password_token)) => {
                // Note: this `prompt` method will echo the password in the console.
                //       Real code might want to use a better way to handle this.
                let hint = password_token.hint().unwrap_or("None");
                let prompt_message = format!("Enter the password (hint {}): ", &hint);
                let password = prompt(prompt_message.as_str())?;

                client
                    .check_password(password_token, password.trim())
                    .await?;
            }
            Ok(_) => (),
            Err(e) => return Err(e.into()),
        };
        match client.session().save_to_file(SESSION_FILE) {
            Ok(_) => {}
            Err(_) => {
                sign_out = true;
            }
        }
    }

    if sign_out {
        // TODO revisit examples and get rid of "handle references" (also, this panics)
        drop(client.sign_out_disconnect().await);
    }

    Ok(client)
}

static DO_NOT_RETRY: AtomicBool = AtomicBool::new(false);

fn user_id_tag(user_id: d::UserId) -> u32 {
    let mut h: u64 = user_id.into();
    h = h.rotate_right(33);
    h = h.wrapping_mul(0xff51afd7ed558ccdu64);
    h = h.rotate_right(33);
    h = h.wrapping_mul(0xc4ceb9fe1a85ec53u64);
    h = h.rotate_right(33);
    return h as u32;
}

pub fn user_id_bot_name(user_id: d::UserId) -> String {
    format!("discogram_{:x}_bot", user_id_tag(user_id))
}

macro_rules! or_cancel {
    ($e:expr, $client:expr, $bot_father:expr) => {
        match $e {
            Ok(v) => v,
            err => {
                let _ = $client.send_message($bot_father, "/cancel").await;
                err?
            }
        }
    };
}
pub const ABOUT: &str = "This bot will do nothing in chats which it is not set up for. It will ignore any direct messages sent to it.";
static CLIENT: OnceCell<Mutex<Client>> = OnceCell::const_new();
static BOT_FATHER: OnceCell<Chat> = OnceCell::const_new();

pub async fn make_bot(user: &d::User) -> Result<String> {
    if DO_NOT_RETRY.load(Ordering::Relaxed) {
        return Err("DO_NOT_RETRY".into());
    }
    let client = CLIENT
        .get_or_try_init(|| async {
            match connect_client().await {
                Ok(client) => Ok(Mutex::new(client)),
                Err(e) => Err(e),
            }
        })
        .await?
        .lock()
        .await;

    const BOT_FATHER_NAME: &str = "BotFather";

    let bot_father = BOT_FATHER
        .get_or_try_init(|| async {
            match client.resolve_username(BOT_FATHER_NAME).await {
                Ok(Some(chat)) => Ok(chat),
                Ok(None) => panic!("BotFather not found"),
                Err(e) => Err(Box::new(e) as Error),
            }
        })
        .await?;

    client.send_message(bot_father, "/newbot").await?;

    let mut token = String::new();
    let bot_username = user_id_bot_name(user.id);
    loop {
        let update = client.next_update().await?;
        println!("{:?}", update);
        match update {
            Update::NewMessage(msg) if msg.sender().map(|s| s.id()) == Some(bot_father.id()) => match msg.text() {
                "Alright, a new bot. How are we going to call it? Please choose a name for your bot." => {
                    or_cancel!(client.send_message(bot_father, user.global_name.as_deref().unwrap_or_else(|| user.name.as_str())).await, client, bot_father);
                }
                "Good. Now let's choose a username for your bot. It must end in `bot`. Like this, for example: TetrisBot or tetris_bot." => {
                    or_cancel!(client.send_message(bot_father, bot_username.as_str()).await, client, bot_father);
                }
                t if t.starts_with("Done! Congratulations on your new bot.") => {
                    token = t[t.find("HTTP API:\n").ok_or("unexpected botfather response")?..]
                        .split_whitespace()
                        .next()
                        .ok_or("unexpected botfather response")?
                        .trim_start_matches("https://api.telegram.org/bot")
                        .to_string();
                    or_cancel!(client.send_message(bot_father, "/setdescription").await, client, bot_father);
                }
                "Choose a bot to change description." | "Choose a bot to change the about section." | "Choose a bot to change profile photo." => {
                    or_cancel!(client.send_message(bot_father, format!("@{bot_username}")).await, client, bot_father);
                }
                "OK. Send me the new description for the bot. People will see this description when they open a chat with your bot, in a block titled 'What can this bot do?'." => {
                    or_cancel!(client.send_message(bot_father, ABOUT).await, client, bot_father);
                }
                "Success! Description updated. You will be able to see the changes within a few minutes. /help" => {
                    or_cancel!(client.send_message(bot_father, "/setabouttext").await, client, bot_father);
                }
                "OK. Send me the new 'About' text. People will see this text on the bot's profile page and it will be sent together with a link to your bot when they share it with someone." => {
                    or_cancel!(client.send_message(bot_father, ABOUT).await, client, bot_father);
                }
                "Success! About section updated. /help" => {
                    or_cancel!(client.send_message(bot_father, "/setuserpic").await, client, bot_father);
                }
                "OK. Send me the new profile photo for the bot." => {
                    or_cancel!(client.send_message(bot_father, InputMessage::from("").photo_url(user.avatar_url().unwrap_or("https://archive.org/download/discordprofilepictures/discordblue.png".to_string()))).await, client, bot_father);
                }
                "Success! Profile photo updated. /help" => {
                    break;
                }
                _=> {
                    log::error!("Unexpected message from bot father: {}", msg.text());
                    or_cancel!(Err("Unexpected message from bot father"), client, bot_father);
                }
            },
            _ => (),
        }
    }
    Ok(token)
}
