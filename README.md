A bot for mirroring Discord and Telegram channels.

Setup instructions (do once) (note that building from source is the only "supported" path (I do not support anything, but at least you have the source code; I am not providing any executables)):
- Install the Rust compiler.
- Clone the repo.
- Create a Discord bot and a Telegram bot. (Instructions not included here.)
- Create a `.env` file and add `DISCORD_TOKEN="<discord bot token>"` and `TELOXIDE_TOKEN="<telegram bot token>"` lines to it. You probably also want `RUST_LOG=info,tracing::span=off,serenity::gateway::shard=off` in there.
- Create a `config.toml` file, then add
  ```
  [options]
  admins = [<your discord id>]
  image_channel = <channel_id>
  ```
  The admins field is [optional] and only currently enables an autocomplete list when running the `/bridge` command (see below). The image_channel is needed for profile pictures in telegram->discord to work. The <channel_id> should be a channel your bot has access to and isn't used for anything else.

  (It's not a problem for the bot if it's used it for something else, but the bot will spam it with telegram profile pictures.)
- Run the bot with `cargo run --release`. Alternatively, you can build the bot and put the executable wherever you want, however the `.env` and `config.toml` files should be in whatever the working directory of the bot is. Note that the bot will also maintain a database of message mappings, which'll be created in the same place. The latter option is untested but I don't see why it wouldn't work.

Usage instructions (for each pair of channels you want to bridge):
- Add the Telegram bot to the Telegram channel and the Discord bot to the Discord channel. On the Telegram side, make sure the bot has read messages permission. On the Discord side, make sure the bot has Manage Messages and Manage Webhooks permissions and is added with scopes `bot` and `applications.commands` (the Oauth link should probably look something like `https://discord.com/oauth2/authorize?client_id=<a bunch of numbers>&permissions=536879104&integration_type=0&scope=bot+applications.commands`).
- Make sure that you have Manage Channel permissions for whatever channel you want to link on Discord.
- Run `/bridge chat: [telegram chat id]` in the Discord channel you want to link. 

  If you did the optional `admins` step, there will be an autocomplete listing all unmapped telegram channels the bot is in (or well, a best-effort guess; if no messages have been sent since the bot was added, it might not be listed, and if the bot was removed, it'll still be listed (to remove a channel that the bot was removed from from the autocomplete list, simply attempt to bridge to it; the command will fail and the channel will not be listed again)). 
  
  Otherwise, you'll have to find the chat id of the Telegram chat some other way. Note that _anyone can add mappings_ as long as they have the appropriate Discord permissions. The `admins` list only controls who sees autocomplete. Correspondingly, if someone gets the add link for your Discord bot and adds it somewhere, finds the @ handle for your Telegram bot and adds that somewhere, they will be able to use your hosting of the bot. This is arguably a denial of service vulnerability. If this bothers you, you can add `if !db::admins().await.contains(&command.user.id) { return; }` to the beginning of `handle_bridge_command` in `main.rs`.
- To remove a bridge, run `/unbridge` on the Discord or Telegram side.
- You can also mark a Discord server or category as a named "hub." Any admin knowing the name can then run `/bridge <hub name>` in a Telegram channel with the bot to create a channel in the server/category linked to the Telegram channel from which the command was run. (There is currently no support for linking to an existing channel from Telegram.) See the `/hub`, `/unhub`, and `/hubinfo` commands on Discord.

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

Frequently Asked Questions (nobody has asked these but they're questions I hypothesize someone might want to ask):
- Q: Does the bot support Discord <-> Discord, Telegram <-> Telegram, or many-one mappings?
  
  A: No, and there are no specific plans to support this. It's not out of the question that it'll happen at some point.
- Q: I want a feature that you don't have, what do I do?
  
  A: I've supported most things that are relevant to my use-case. Some others are vaguely on the list. You are welcome to either request features or make a pull-request adding support for the feature.
- Q: Why are there no comments in the code?
  
  A: Because perfect code is self-documenting. This code is not perfect, but I didn't plan to share it with anyone, and so far, I have had no problems editing it myself despite there being no comments. If you want to make changes, good luck!

- Q: Why are there _a few_ comments in the code?

  A: Claude likes to write inane comments, some of which I kept. A few of the comments are from copy-pasting example code.
