use serenity::client::Client;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::model::id::ChannelId;
use serenity::prelude::{EventHandler, Context, TypeMapKey};
use serenity::framework::standard::{
    StandardFramework,
    CommandResult,
    macros::{
        command,
        group
    }
};
use serenity::http::client::Http;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;
use tokio::sync::mpsc::*;
use regex::Regex;
use std::time::{Duration, Instant};
use srl_http::{Entrants};

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Config {
    token: String,
    prefix: String,
    ircnick: String,
    ircpass: String,
}

#[derive(Clone)]
struct ContextWrapper {
    http: Arc<Http>,
    channel_id: ChannelId,
}


impl std::fmt::Debug for ContextWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Context")
    }
}

#[group]
#[commands(ping, startrace, roll)]
struct General;

struct Chan;

impl TypeMapKey for Chan {
    type Value = tokio::sync::mpsc::Sender<(ContextWrapper,Message,RaceConfig)>;
}

struct Handler;

impl EventHandler for Handler {
    fn ready(&self, _ctx: Context, bot: Ready)
    {
        println!("Connected to Discord as {}", bot.user.tag());
    }
}

/*
static RACEBOT:     &str = "dagit";
static SRL:         &str = "dagit";
const  RACEBOTWAIT: u64  = 10;
*/
static RACEBOT:     &str = "RaceBot";
static SRL:         &str = "#speedrunslive";
const  RACEBOTWAIT: u64  = 3;
static SRL_API:     &str = "http://api.speedrunslive.com:81/";
// Ex. response from RaceBot: "Race initiated for Mega Man Hacks. Join #srl-dr0nu to participate."
//static ROOM: &str = r"Race initiated for Mega Man Hacks\. Join (?P<chan>\#srl\-[[:alnum:]]+) to participate\.";
static ROOM: &str = r"Race initiated for (?P<game>[ [:alnum:]]+)\. Join (?P<chan>\#srl\-[[:alnum:]]+) to participate\.";
//static ROOM: &str = r"Race initiated.*Mega Man Hacks.*Join.*(?P<chan>\#srl\-[[:alnum:]]+).*";

#[derive(Debug,Copy,Clone)]
struct RaceConfig {
    pub guild:     &'static str,  // 'MM2 Randomizer'
    pub game_code: &'static str,  // 'megamanhacks'
    pub game_name: &'static str,  // 'Mega Man Hacks'
    pub race_goal: &'static str,  // 'mega man 2 randomizer - any% (easy)'
}

static RACECONFIGS: &'static [RaceConfig] = &[
    // The mega man 2 randomizer discord
    RaceConfig {
        guild:     "Mega Man 2 Randomizer",
        game_code: "megamanhacks",
        game_name: "Mega Man Hacks",
        race_goal: "mega man 2 randomizer - any% (easy)",
    },
    // The mega man 2 randomizer tournament discord
    RaceConfig {
        guild:     "Mega Man 2 Randomizer Tournament",
        game_code: "megamanhacks",
        game_name: "Mega Man Hacks",
        race_goal: "mega man 2 randomizer - any% (easy)",
    },
    // The mega man 9 tournament discord
    RaceConfig {
        guild:     "Mega Man 9 Tournament",
        game_code: "mm9",
        game_name: "Mega Man 9",
        race_goal: "any%",
    },
    RaceConfig {
        guild:     "whats this",
        game_code: "megamanhacks",
        game_name: "Mega Man Hacks",
        race_goal: "mega man 2 randomizer - any% (easy)",
    },
];

fn main() {
    let config_reader = BufReader::new(File::open("config.json").expect("Failed opening file"));
    let config: Config = serde_json::from_reader(config_reader).expect("Failed decoding config");
    let irccfg = config.clone();

    let (sender, mut receiver) = channel(10);
    let (ctx_sender, ctx_receiver)
        :(std::sync::mpsc::Sender<(ContextWrapper,Instant,RaceConfig)>
         ,std::sync::mpsc::Receiver<(ContextWrapper,Instant,RaceConfig)>)
        = std::sync::mpsc::channel();
    let (to_discord, for_discord)
        :(std::sync::mpsc::Sender<(ContextWrapper,String)>
         ,std::sync::mpsc::Receiver<(ContextWrapper,String)>)
         = std::sync::mpsc::channel();

    let to_discord2 = to_discord.clone();
         
    let discord = std::thread::spawn(move || {
        let mut client = Client::new(&config.token, Handler)
            .expect("Error creating client");
        {
            let mut data = client.data.write();
            data.insert::<Chan>(sender);
        }
        client.with_framework(StandardFramework::new()
            .configure(|c| c.prefix(&config.prefix))
            .group(&GENERAL_GROUP));

        // start listening for events by starting a single shard
        if let Err(why) = client.start_autosharded() {
            println!("An error occured while running the client: {:?}", why);
        }
    });
    // We need a separate thread here because serenty will try to thread::sleep() and that
    // breaks tokio (which is used by the irc library.
    let irc_messages = std::thread::spawn(move || {
        loop {
            let (ctx, msg) = for_discord.recv().expect("failed to get message from IRC thread");
            ctx.channel_id.say(&ctx.http, msg).expect("Failed to respond on discord");
        }
    });
    let irc = std::thread::spawn(move || {
        let thread = async move {
            use futures::select;
            use futures::prelude::*;
            use irc::client::prelude::*;
            let config = Config {
                nickname: Some(irccfg.ircnick.to_owned()),
                nick_password: Some(irccfg.ircpass.to_owned()),
                server: Some("irc.speedrunslive.com".to_owned()),
                channels: vec!["#speedrunslive".to_owned()],
                ..irc::client::prelude::Config::default()
            };
            let godot1 = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)); // Not currently waiting on godot
            let godot2 = godot1.clone();

            let mut client = Client::from_config(config).await.expect("Failed to connect to SRL");
            client.identify().expect("Failed to identify with nickserv");
            println!("Connected to IRC as {}", client.current_nickname());
            let send_client = client.sender();
            let reactor1 = async move {

                let mut irc_stream = client.stream().expect("failed to make irc stream");
                loop {
                    let irc_msg = irc_stream.select_next_some().await.expect("failed to get next message");
                    let m = match irc_msg.clone().command {
                        Command::PRIVMSG(channel, message) => Some((channel, message)),
                        Command::NOTICE(channel, message) => Some((channel, message)),
                        _ => None,
                    };
                    if let Some((channel, message)) = m {
                        // This substition will scrub all the control characters from the input
                        // https://modern.ircdocs.horse/formatting.html
                        // https://www.debuggex.com/r/5mzH8NGlLB6RyqaL
                        let scrub = r"\x03([0-9]{1,2}(,[0-9]{1,2})?)?|\x04[a-fA-F0-9]{6}|[\x02\x0f\x11\x16\x1d\x1e\x1f]";
                        let scrub_re = Regex::new(scrub).expect("Failed to build scrub");
                        let message = scrub_re.replace_all(&message, "");
                        println!("channel = {}, message = '{}'", channel, message);
                        // Check to see if we should note down the channel
                        if irc_msg.source_nickname() == Some(&RACEBOT) {
                            if godot1.load(std::sync::atomic::Ordering::SeqCst) {
                                let re = Regex::new(ROOM).expect("Failed to build regex");
                                println!("state: waiting on godot, message = '{}'", &message);

                                if message == "You've already started a race. Please use .setgame in the race channel if you need to set it to the correct game."{
                                    godot1.store(false, std::sync::atomic::Ordering::SeqCst);
                                    match ctx_receiver.try_iter().last() {
                                        Some((ctx,_,_)) => {
                                            to_discord.send((ctx, message.to_string())).expect("Failed to send to discord");
                                        },
                                        _ => { println!("Timetout failed");
                                        }
                                    };
                                } else {
                                    let chan = re.captures(&message)
                                        .map(|c| c.name("chan"));
                                    let game = re.captures(&message)
                                        .map(|c| c.name("game"));
                                    match chan.and_then(std::convert::identity) {
                                        None => (),
                                        Some(c) => {
                                            godot1.store(false, std::sync::atomic::Ordering::SeqCst);
                                            match ctx_receiver.try_iter().last() {
                                                Some((ctx,sent_at,config)) => {
                                                    if sent_at.elapsed() < Duration::from_secs(RACEBOTWAIT) {
                                                        let chan = c.as_str();
                                                        let game = game.and_then(std::convert::identity).map(|g| g.as_str()).unwrap_or("unknown");
                                                        println!("chan is '{}'", chan);
                                                        println!("game is '{}'", game);
                                                        if game == config.game_name {
                                                            println!("responding on discord");
                                                            to_discord.send((ctx, format!("/join {}", &chan))).expect("failed to send to irc_message thread");
                                                            //ctx.channel_id.say(&ctx.http, format!("/join {}", &chan)).expect("Failed to respond on discord");
                                                            println!("joining {}", &chan);
                                                            client.send_join(&chan).expect("Failed to join race channel");
                                                            println!("setting goal");
                                                            client.send_privmsg(&chan, format!(".setgoal {}",config.race_goal)).expect("Failed to setgoal");
                                                            client.send_privmsg(&chan, ".enter").expect("Failed to enter");
                                                        }
                                                    } else { // this event took too long to arrive, it's probably
                                                             // not for us.
                                                        println!("channel creation event arrived late");
                                                    }
                                                },
                                                _ => {
                                                    println!("Timeout failed");
                                                }
                                            }
                                        },
                                    };
                                }
                            } else {
                                // Join any mega man hacks race room and just chill
                                let re = Regex::new(ROOM).expect("Failed to build regex");
                                println!("state: not waiting, message = '{}'", &message);
                                let chan = re.captures(&message)
                                    .map(|c| c.name("chan"));
                                let game = re.captures(&message)
                                    .map(|c| c.name("game"));
                                match chan.and_then(std::convert::identity) {
                                    None => (),
                                    Some(c) => {
                                        let game = game.and_then(std::convert::identity).map(|g| g.as_str()).unwrap_or("unknown");
                                        for rc in RACECONFIGS {
                                            if rc.game_name == game {
                                                println!("joining {}", c.as_str());
                                                client.send_join(&c.as_str()).expect("Failed to join race channel");
                                                continue;
                                            }
                                        }
                                    },
                                };
                            }
                        }
                        if message.starts_with (".entrants") {
                            let split = message.split(" ").collect::<Vec<_>>();
                            if split.len() > 1 {
                                let entrants : Entrants = srl_http::entrants(SRL_API, &split[1]).await.expect("Failed to get race entrants");
                                println!("{:#?}", entrants);
                            }
                        }
                        if message.starts_with(".botenter") || message.starts_with(".botjoin") {
                            let ch = irc_msg.response_target().unwrap_or(&channel);
                            client.send_privmsg(&ch, ".enter").expect("Failed send_privmsg");
                        }
                        if message.starts_with(".botquit") || message.starts_with(".botforfeit") {
                            let ch = irc_msg.response_target().unwrap_or(&channel);
                            client.send_privmsg(&ch, ".quit").expect("Failed send_privmsg");
                        }
                        if message.starts_with(".roll") {
                            use rand::Rng;
                            let ch = irc_msg.response_target().unwrap_or(&channel);
                            let seed: i32 = rand::thread_rng().gen_range(0, i32::max_value());
                            let alpha_seed = convert_to_base26(seed);
                            client.send_privmsg(&ch, format!("Your seed is: {}", alpha_seed)).expect("Failed send_privmesg");
                        }
                        if message.starts_with(".multi") {
                            let ch = irc_msg.response_target().unwrap_or(&channel);
                            let prefix = "#srl-";
                            if ch.starts_with(prefix) {
                                let (_,raceid) = ch.split_at(prefix.len());
                                let entrants : Entrants = srl_http::entrants(SRL_API, raceid).await.expect("failed to get entrants");
                                let twitches = entrants
                                    .entrants
                                    .iter()
                                    .map(|(_,v)| v.twitch.to_owned())
                                    .filter(|n| n.len() > 0)
                                    .collect::<Vec<_>>();
                                let mut url = "http://multitwitch.tv/".to_owned();
                                url.push_str(&twitches.join("/"));
                                if twitches.len() == 0 {
                                    client.send_privmsg(&ch, "Sorry, no registered streams").expect("Failed to send_privmsg");
                                } else {
                                    client.send_privmsg(&ch, format!("Multitwitch URL: {}", &url)).expect("Failed to send_privmsg");
                                }

                            }
                        }
                        if message.contains(client.current_nickname()) {
                            let ch = irc_msg.response_target().unwrap_or(&channel);
                            client.send_privmsg(&ch, "beep boop").expect("Failed send_privmsg");
                        }
                    }
                }
            };
            let reactor2 = async move {
                loop {
                    match receiver.recv().await {
                        None => { /* TODO: should we exit or what? */}
                        Some((ctx, _msg, config)) => {
                            let godot2 = godot2.clone();
                            let godot3 = godot2.clone();
                            let ctx2 = ctx.clone();
                            ctx_sender.send((ctx,Instant::now(),config)).expect("Failed to send ctx");
                            println!("set godot2 true");
                            godot2.store(true, std::sync::atomic::Ordering::SeqCst);
                            println!("starting race");
                            send_client.send(Command::PRIVMSG(SRL.to_string(), format!(".startrace {}", config.game_code))).expect("Failed to startrace");
                            tokio::time::delay_for(Duration::from_secs(RACEBOTWAIT)).await;
                            if godot3.load(std::sync::atomic::Ordering::SeqCst) {
                                godot3.store(false, std::sync::atomic::Ordering::SeqCst);
                                println!("Letting discord know we failed to get a channel in time");
                                to_discord2.send((ctx2, "Sorry, something went wrong. Check IRC, you might have a channel waiting.".to_string()))
                                    .expect("Failed to respond on discord");
                            }
                        }
                    }
                }
            };
            use futures::pin_mut;
            let r1 = reactor1.fuse();
            let r2 = reactor2.fuse();
            pin_mut!(r1, r2);
            select!{
                () = r1 => {},
                () = r2 => {}
            };
        };
        let mut rt = tokio::runtime::Builder::new()
            .basic_scheduler()
            .enable_all()
            .build()
            .expect("couldn't build tokio runtime");
        rt.block_on(thread);
    });
    irc.join().expect("irc thread exited");
    discord.join().expect("Discord thread exited");
    irc_messages.join().expect("irc_messages thread exited");
}

#[command]
fn ping(ctx: &mut Context, msg: &Message) -> CommandResult {
    if msg.author.bot { return Ok(()) } // Don't respond to bots
    let mut details = match msg.guild(&ctx.cache) {
        None => "unknown guild".to_owned(),
        Some(g) => {
            format!("From guild with name '{}'", g.read().name)
        },
    };
    details.push_str(format!("\nFrom user with display_name '{}'", msg.author.name).as_str());
    if let Some(cname) = msg.channel_id.name(&ctx.cache)
    {
        details.push_str(format!("\nFrom channel '{}'", cname).as_str());
    }
    msg.reply(ctx, format!("Ping received.\n{}", details))?;
    Ok(())
}

#[command]
fn roll(ctx: &mut Context, msg: &Message) -> CommandResult {
    use rand::Rng;
    if msg.author.bot { return Ok(()) } // Don't respond to bots
    let seed: i32 = rand::thread_rng().gen_range(0, i32::max_value());
    let alpha_seed = convert_to_base26(seed);
    msg.reply(ctx, format!("Your seed is: {}", alpha_seed))?;
    Ok(())
}

#[command]
fn startrace(ctx: &mut Context, msg: &Message) -> CommandResult {
    if msg.author.bot { return Ok(()) } // Don't respond to bots
    // send a message to IRC
    let mut data = ctx.data.write();
    let chan = data.get_mut::<Chan>().unwrap();
    let new_ctx = ContextWrapper {
        http: ctx.http.clone(),
        channel_id: msg.channel_id,
    };
    let guild = msg.guild(&ctx.cache).map(|g| g.read().name.to_owned());
    if let Some(guild) = guild {
        for rc in RACECONFIGS {
            if rc.guild == guild {
                msg.reply(&ctx, "Attempting to start race...")?;
                (*chan).try_send((new_ctx,msg.clone(),*rc))?;
                break;
            }
        }
    }
    Ok(())
}

fn convert_to_base26(seed: i32) -> String
{
    let base26 = (b'A'..=b'Z').map(char::from).collect::<Vec<_>>();
    let mut output = String::from("");
    let base = base26.len() as i32;
    let mut i: i32 = seed;
    while i > 0 {
        output.push(base26[(i % base) as usize]);
        i /= base;
    }
    output.chars().rev().collect()
}
