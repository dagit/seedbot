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
use serenity::http::raw::Http;
use serde::{Deserialize, Serialize};
use irc::client::prelude::*;
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;
use tokio::sync::mpsc::*;
use regex::Regex;
use std::time::{Duration, Instant};
use std::collections::{VecDeque,HashMap};

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


group!({
    name: "general",
    options: {},
    commands: [ping, startrace, roll],
});

struct Chan;

impl TypeMapKey for Chan {
    type Value = Sender<(ContextWrapper,Message,RaceConfig)>;
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

#[derive(Clone)]
pub enum MultiState {
    Idle,
    AwaitingEntrantsResponse(String),
    AwaitingURL(VecDeque<String> /* the unknown entrants */, String /* race channel */, Vec<String> /* channel names */), // Let's assume no more than 256 entrants in a race
}

fn main() {
    let config_reader = BufReader::new(File::open("config.json").expect("Failed opening file"));
    let config: Config = serde_json::from_reader(config_reader).expect("Failed decoding config");
    let irccfg = config.clone();

    let (sender, receiver) = channel(10);
    let (ctx_sender, ctx_receiver)
        :(std::sync::mpsc::Sender<(ContextWrapper,Instant,RaceConfig)>
         ,std::sync::mpsc::Receiver<(ContextWrapper,Instant,RaceConfig)>)
        = std::sync::mpsc::channel();

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
    let irc = std::thread::spawn(move || {
        let config = irc::client::prelude::Config {
            nickname: Some(irccfg.ircnick.to_owned()),
            nick_password: Some(irccfg.ircpass.to_owned()),
            server: Some("irc.speedrunslive.com".to_owned()),
            channels: Some(vec!["#speedrunslive".to_owned()]),
            ..irc::client::prelude::Config::default()
        };

        let mut reactor = IrcReactor::new().expect("Failed to create IRC reactor");
        let client = reactor.prepare_client_and_connect(&config).expect("Failed to connect to SRL");
        client.identify().expect("Failed to identify with nickserv");
        println!("Connected to IRC as {}", client.current_nickname());
        let send_client = client.clone();

        let godot1 = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)); // Not currently waiting on godot
        let godot2 = godot1.clone();

        let mut multi_state : MultiState = MultiState::Idle;

        // Maps IRC nick to twitch channel name
        let mut stream_map : HashMap<String, Option<String>> = HashMap::new();

        reactor.register_client_with_handler(client, move |client, irc_msg| {
            //print!("{}", irc_msg);
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
                let scrub_re = Regex::new(scrub).expect("Failed to build scub");
                let message = scrub_re.replace_all(&message, "");
                println!("channel = {}, message = '{}'", channel, message);
                // Check to see if we should note down the channel
                if irc_msg.source_nickname() == Some(&RACEBOT) {
                    if godot1.load(std::sync::atomic::Ordering::SeqCst) {
                        let re = Regex::new(ROOM).expect("Failed to build regex");
                        println!("state: waiting on godot, message = '{}'", &message);
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
                                                ctx.channel_id.say(&ctx.http, format!("/join {}", &chan)).map_err(|e| {
                                                    IrcError::Custom{ inner: Error::from_boxed_compat(Box::new(e))}
                                                }).expect("Failed to respond on discord");
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
                                        break;
                                    }
                                }
                            },
                        };
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
                match multi_state.clone() {
                    MultiState::Idle => {
                        if message.starts_with(".multi") {
                            let ch = irc_msg.response_target().unwrap_or(&channel);
                            client.send_privmsg(&ch, ".entrants").expect("Failed send_privmsg");
                            println!("entering state: awaiting entrants response");
                            multi_state = MultiState::AwaitingEntrantsResponse(ch.to_owned());
                        }
                    },
                    MultiState::AwaitingEntrantsResponse(race_channel) => {
                        println!("awaiting entrants response");
                        // :RaceBot!RaceBot@1523E686.9748F92.176EC33F.IP NOTICE rollchan :dagit | rollchan
                        let source = irc_msg.source_nickname().unwrap_or("");
                        if channel == irccfg.ircnick && source == RACEBOT {
                            let entrants = parse_entrants(&message);
                            let mut unmapped_entrants: VecDeque<String> = VecDeque::new();
                            let mut race_entrants:Vec<String> = vec![];
                            println!("parsed {} entrants", entrants.len());
                            for e in entrants {
                                match stream_map.get(e) {
                                    Some(Some(s)) => race_entrants.push(s.to_owned()),
                                    Some(None)    => (), // This means we've queried this user in the past
                                                         // and they had no stream.
                                    None          => unmapped_entrants.push_back(e.to_owned()),
                                }
                            }
                            let num_to_query = unmapped_entrants.len() as u8;
                            multi_state = MultiState::AwaitingURL(unmapped_entrants.clone(), race_channel.to_owned(), race_entrants);
                            println!("entering state: awaiting url {}", num_to_query);
                            for entrant in &unmapped_entrants {
                                client.send_privmsg(&race_channel, format!(".stream {}", entrant)).expect("Failed send_privmsg");
                            }
                        } else {
                            // Something went wrong
                            println!("entering state: idle (something went wrong)");
                            multi_state = MultiState::Idle;
                        }
                        
                    },
                    MultiState::AwaitingURL(mut unmapped_entrants, race_channel, mut race_entrants) => {
                        println!("awaiting url {}", unmapped_entrants.len());
                        //let ch = irc_msg.response_target().unwrap_or(&channel);
                        let source = irc_msg.source_nickname().unwrap_or("");
                        let http_schema = r"^https?://[.[:alnum:]]*twitch.tv/(?P<twitchchannel>.*)";
                        let http_schema_re = Regex::new(http_schema).expect("Failed to build http_schema");

                        println!("source = {}", source);
                        if source == RACEBOT && http_schema_re.is_match(&message) {
                            println!("match a URL");
                            let ircnick = unmapped_entrants.pop_front();
                            let num = unmapped_entrants.len();
                            println!("entering state: awaiting url {}", num);
                            let twitchchannel = http_schema_re.captures(&message)
                                .map(|c| c.name("twitchchannel"))
                                .and_then(std::convert::identity)
                                .map(|g| g.as_str());
                            if let Some(twitchchannel) = twitchchannel {
                                println!("twitchannel = {}", twitchchannel);
                                if let Some(ircnick) = ircnick {
                                    println!("ircnick = {}", ircnick);
                                    println!("insert: {} => {}", ircnick, twitchchannel);
                                    stream_map.insert(ircnick, Some(twitchchannel.to_string()));
                                    race_entrants.push(twitchchannel.to_string());
                                }
                            } else {
                                println!("unable to parse twitch channel: {}", message);
                            }
                            multi_state = MultiState::AwaitingURL(unmapped_entrants, race_channel.to_owned(), race_entrants);
                        } else if source == RACEBOT && message == "Doesn't exist." {
                            println!("no stream set");
                            let ircnick = unmapped_entrants.pop_front();
                            multi_state = MultiState::AwaitingURL(unmapped_entrants, race_channel.to_owned(), race_entrants);
                            if let Some(ircnick) = ircnick {
                                println!("ircnick = {}, caching no channel", ircnick);
                                stream_map.insert(ircnick, None);
                            }
                        } else { // this is a failsafe case for when a URL is registered but doesn't parse
                            println!("failed to match a URL");
                            let ircnick = unmapped_entrants.pop_front();
                            multi_state = MultiState::AwaitingURL(unmapped_entrants, race_channel.to_owned(), race_entrants);
                            if let Some(ircnick) = ircnick {
                                println!("ircnick = {}, caching no channel", ircnick);
                                stream_map.insert(ircnick, None);
                            }
                        }
                        if let MultiState::AwaitingURL(ref unmapped, _, ref entrants) = multi_state.clone() {
                            if unmapped.len() == 0 {
                                println!("entering state: idle (done)");
                                multi_state = MultiState::Idle;
                                let mut url = "http://multitwitch.tv/".to_owned();
                                url.push_str(&entrants.join("/"));
                                if entrants.len() == 0 {
                                    client.send_privmsg(&race_channel, "Sorry, no registered streams").expect("Failed to send_privmsg");
                                } else {
                                    client.send_privmsg(&race_channel, format!("Multitwitch URL: {}", &url)).expect("Failed to send_privmsg");
                                }
                            }
                        }
                    },
                }
                if message.starts_with(".deletecache") {
                    stream_map.clear();
                }
                if message.contains(client.current_nickname()) {
                    let ch = irc_msg.response_target().unwrap_or(&channel);
                    client.send_privmsg(&ch, "beep boop").expect("Failed send_privmsg");
                }
            }
            Ok(())
        });
        use irc::error::IrcError;
        use failure::Error;
        use futures::future::{ok};
        use tokio::prelude::*;
        reactor.register_future(receiver.map_err(|e| {
            IrcError::Custom{ inner: Error::from_boxed_compat(Box::new(e)) }
        }).for_each(move |(ctx,_msg,config)| {
            let godot2 = godot2.clone();
            let godot3 = godot2.clone();
            let send_client = send_client.clone();
            let ctx2 = ctx.clone();
            ctx_sender.send((ctx,Instant::now(),config)).expect("Failed to send ctx");
            println!("set godot2 true");
            godot2.store(true, std::sync::atomic::Ordering::SeqCst);
            println!("starting race");
            send_client.send_privmsg(SRL, format!(".startrace {}", config.game_code)).expect("Failed to startrace");
            std::thread::spawn(move||{
                std::thread::sleep(Duration::from_secs(RACEBOTWAIT));
                if godot3.load(std::sync::atomic::Ordering::SeqCst) {
                    godot3.store(false, std::sync::atomic::Ordering::SeqCst);
                    println!("Letting discord know we failed to get a channel in time");
                    ctx2.channel_id.say(&ctx2.http, "Sorry, something went wrong. Check IRC, you might have a channel waiting.").map_err(|e| {
                        IrcError::Custom{ inner: Error::from_boxed_compat(Box::new(e))}
                    }).expect("Failed to respond on discord");
                }
            });
            ok(())
        }));
        reactor.run().unwrap();
    });
    discord.join().expect("Discord thread exited");
    irc.join().expect("irc thread exited");
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

fn parse_entrants(msg: &str) -> Vec<&str> {
    let mut entrants = vec![];
    for e in msg.split(" | ").map(|e| e.split(" ").nth(0)) {
        match e {
            None => (),
            Some(e) => entrants.push(e),
        }
    };
    entrants
}
