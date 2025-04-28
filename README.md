A bot for mirroring Discord and Telegram channels.

Setup instructions (do once):
- Install the Rust compiler.
- Clone the repo.
- Modify the `const DISCORD_IMAGE_CHANNEL: d::ChannelId = d::ChannelId::new(1267352463158153216);` line in `main.rs` to be a channel your bot has access to and isn't used for anything else (this'll be made a config parameter at some point).
- Create a Discord bot and a Telegram bot.
- Create a `.env` file and add `DISCORD_TOKEN="<discord bot token>"` and `TELOXIDE_TOKEN="<telegram bot token>"` lines to it. You probably also want `RUST_LOG=info,tracing::span=off,serenity::gateway::shard=off` in there.
- [Optional] Create a `config.toml` file, then add
  ```
  [admins]
  users = [<your discord id>]
  ```
  The only this currently affects is that it will enable an autocomplete list when running the `/bridge` command (see below).
- Run the bot with `cargo run --release`. Alternatively, you can build the bot and put the executable wherever you want, however the `.env` and `config.toml` files should be in whatever the working directory of the bot is. Note that the bot will also maintain a database of message mappings, which'll be created in the same place. The latter option is untested but I don't see why it wouldn't work.

Usage instructions (for each pair of channels you want to bridge):
- Add the Telegram bot to the Telegram channel and the Discord bot to the Discord channel. On the Telegram side, make sure the bot has read messages permission. On the Discord side, make sure the bot has Manage Messages and Manage Webhooks permissions and is added with scopes `bot` and `applications.commands` (the Oauth link should probably look something like `https://discord.com/oauth2/authorize?client_id=<a bunch of numbers>&permissions=536879104&integration_type=0&scope=bot+applications.commands`).
- Make sure that you have Manage Channel permissions for whatever channel you want to link on Discord.
- Run `/bridge chat: [telegram chat id]` in the Discord channel you want to link. If you did the optional `admins` step, there will be an autocomplete listing all telegram channels the bot is in (or well, a best-effort guess; if no messages have been sent since the bot was added, it might not be listed, and if the bot was removed, it'll still be listed (to remove a channel that the bot was removed from from the autocomplete list, simply attempt to bridge to it; the command will fail and the channel will not be listed again)). Otherwise, you'll have to find the chat id of the Telegram chat some other way. Note that _anyone can add mappings_ as long as they have the appropriate Discord permissions. The `admins` list only controls who sees autocomplete. Correspondingly, if someone gets the add link for your Discord bot and adds it somewhere, finds the @ handle for your Telegram bot and adds that somewhere, they will be able to use your hosting of the bot. This is arguably a denial of service vulnerability probably, but I don't really care. If this bothers you, you can add `if !db::admins().await.contains(&command.user.id) { return; }` to the beginning of `handle_bridge_command` in `main.rs` or whatever.
- To remove a bridge, run `/unbridge` on the Discord side.
