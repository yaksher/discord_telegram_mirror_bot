A bot for mirroring Discord and Telegram channels.

Setup instructions (do once) (note that building from source is the only supported path):
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

Please note that the features may not be up to date because I may add things and forget to update it. Also the list may not be exhaustive because it was written off the top of my head.

Features (Telegram -> Discord):
- Normal messages (including images, videos, files, etc.). Formatting is supported, where an analog is supported on Discord, though sometimes may have minor issues.
- Replies (including quoting specific text, but not including cross-chat replies). Replies will have a link back to the original message if it's one the bot is aware of, or just a copy of the text if it's a message the bot doesn't have a mapping for.
- Forwarded messages
- Edits (deletions are unsupported because Telegram does not send bots message deletion events; edit a message to `.` in order to delete it on Discord)
- Pins (assuming the bot knows of the corresponding Discord message; note that unpinning is not currently supported)
- Reactions
- Stickers (sent as images)
- Polls (support limited: the poll is sent as a message with preplaced reactions, which are then forwarded the same way as any other reactions.)
- Everything forwarded to Discord is done via webhooks, displaying the profile pictures and names of the Telegram sender.

Features (Discord -> Telegram):
- Normal messages (including images, videos, files, etc.). Formatting is supported, where an analog is support on Telegram, though sometimes may have minor issues due to Discord having its own custom markdown syntax no good parsers exist for.
- Replies. Replies will be a Telegram reply back to the original message if it's one the bot is aware of, or just a copy of the text if it's a message the bot doesn't have a mapping for.
- Forwarded messages
- Edits and deletions
- Reactions (custom emote reactions will just be sent as names)
- Stickers (sent as images) (Lottie format ones, including most of Discord's built-in ones, are unsupported; the bot will notify you if you send an unsupported one.)

Note that polls and pins are not forwarded Discord -> Telegram.

Frequently Ask Questions (not really; nobody has asked these but they're questions I ask myself):
- Q: What the fuck is up with step 3 of the setup instructions?
  A: Webhook messages require a link to the profile picture. I realized later that this can be a special attachment link which refers to one of the attachments on the message, but initially did not realize this; because Telegram does not provide a persistent link for profile pictures (the fetch URL uses the bot's token), I decided to use a Discord channel to host the images. This has the advantage of saving bandwidth for the bot, so I'm not going to change it.
- Q: Does the bot support Discord <-> Discord, Telegram <-> Telegram, or many-one mappings?
  A: No, and there are no specific plans to support this, because it would be obnoxious. I might though.
- Q: I want a feature that you don't have, what do I do?
  A: I've supported most things that are relevant to my use-case. Some others are vaguely on the list. You are welcome to either request features or make a pull-request adding support for the feature (or just modify the code, but I'd appreciate if you make a pull-request since you're using my code).
- Q: Why are there no comments in the code?
  A: Because perfect code is self-documenting. This code is not perfect, but I didn't plan to share it with anyone, and so far, I have had no problems editing it myself despite there being no comments. If you want to make changes, good luck!