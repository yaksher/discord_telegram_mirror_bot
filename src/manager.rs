use grammers_client::session::Session;
use grammers_client::{types::Chat, Client, Config, SignInError};
use std::env;
use std::io::{self, BufRead as _, BufReader, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, OnceCell};

use crate::discord as d;
use crate::telegram as t;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

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
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
        .and_then(|v| {
            v.parse::<i32>()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
        })
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
            Err(e) => {
                println!("NOTE: failed to save the session, will sign out when done: {e}");
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

pub async fn make_bot(user_id: d::UserId) -> Result<t::Bot> {
    static CLIENT: OnceCell<Mutex<Client>> = OnceCell::const_new();
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

    static BOT_FATHER: OnceCell<Chat> = OnceCell::const_new();
    let bot_father = BOT_FATHER
        .get_or_try_init(|| async {
            match client.resolve_username(BOT_FATHER_NAME).await {
                Ok(Some(chat)) => Ok(chat),
                Ok(None) => panic!("BotFather not found"),
                Err(e) => Err(Box::new(e) as Box<dyn std::error::Error>),
            }
        })
        .await?;

    client.send_message(bot_father, "/newbot").await?;
    loop {
        let update = client.next_update().await?;
        println!("{:?}", update);
    }
    Err("".into())
}
