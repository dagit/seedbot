use regex::Regex;
use serde::{Deserialize, Serialize};
use serenity::client::Client;
use serenity::framework::standard::{
    macros::{command, group},
    CommandResult, StandardFramework,
};
use serenity::http::client::Http;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::model::id::ChannelId;
use serenity::prelude::{Context, EventHandler, TypeMapKey};
use srl_http::Entrants;
use std::collections::VecDeque;
use std::fs::File;
use std::io::BufReader;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
#[commands(ping, startrace, roll, lottery)]
struct General;

struct Chan;

impl TypeMapKey for Chan {
    type Value = Mutex<Sender<(ContextWrapper, Message, RaceConfig)>>;
}

struct Handler;

impl EventHandler for Handler {
    fn ready(&self, _ctx: Context, bot: Ready) {
        println!("Connected to Discord as {}", bot.user.tag());
    }
}

/*
static RACEBOT:     &str = "dagit";
static SRL:         &str = "dagit";
const  RACEBOTWAIT: u64  = 10;
*/
static RACEBOT: &str = "RaceBot";
static SRL: &str = "#speedrunslive";
const RACEBOTWAIT: u64 = 3;
static SRL_API: &str = "http://api.speedrunslive.com:81/";
// Ex. response from RaceBot: "Race initiated for Mega Man Hacks. Join #srl-dr0nu to participate."
//static ROOM: &str = r"Race initiated for Mega Man Hacks\. Join (?P<chan>\#srl\-[[:alnum:]]+) to participate\.";
static ROOM: &str = r"Race initiated for (?P<game>[ [:alnum:]]+)\. Join (?P<chan>\#srl\-[[:alnum:]]+) to participate\.";
//static ROOM: &str = r"Race initiated.*Mega Man Hacks.*Join.*(?P<chan>\#srl\-[[:alnum:]]+).*";

#[derive(Debug, Copy, Clone)]
struct RaceConfig {
    pub guild: &'static str,     // 'MM2 Randomizer'
    pub game_code: &'static str, // 'megamanhacks'
    pub game_name: &'static str, // 'Mega Man Hacks'
    pub race_goal: &'static str, // 'mega man 2 randomizer - any% (easy)'
}

static RACECONFIGS: &'static [RaceConfig] = &[
    // The mega man 2 randomizer discord
    RaceConfig {
        guild: "Mega Man 2 Randomizer",
        game_code: "megamanhacks",
        game_name: "Mega Man Hacks",
        race_goal: "mega man 2 randomizer - any% (easy)",
    },
    // The mega man 2 randomizer tournament discord
    RaceConfig {
        guild: "Mega Man 2 Randomizer Tournament",
        game_code: "megamanhacks",
        game_name: "Mega Man Hacks",
        race_goal: "mega man 2 randomizer - any% (easy)",
    },
    // The mega man 9 tournament discord
    RaceConfig {
        guild: "Mega Man 9 Tournament",
        game_code: "mm9",
        game_name: "Mega Man 9",
        race_goal: "any%",
    },
    RaceConfig {
        guild: "whats this",
        game_code: "megamanhacks",
        game_name: "Mega Man Hacks",
        race_goal: "mega man 2 randomizer - any% (easy)",
    },
];

fn main() {
    let config_reader = BufReader::new(File::open("config.json").expect("Failed opening file"));
    let config: Config = serde_json::from_reader(config_reader).expect("Failed decoding config");
    let irccfg = config.clone();

    let (sender, receiver) = std::sync::mpsc::channel();
    let (to_discord, for_discord): (
        std::sync::mpsc::Sender<(ContextWrapper, String)>,
        std::sync::mpsc::Receiver<(ContextWrapper, String)>,
    ) = std::sync::mpsc::channel();

    let discord = std::thread::spawn(move || {
        let mut client = Client::new(&config.token, Handler).expect("Error creating client");
        {
            let mut data = client.data.write();
            data.insert::<Chan>(Mutex::new(sender));
        }
        client.with_framework(
            StandardFramework::new()
                .configure(|c| c.prefix(&config.prefix))
                .group(&GENERAL_GROUP),
        );

        // start listening for events by starting a single shard
        if let Err(why) = client.start_autosharded() {
            println!("An error occured while running the client: {:?}", why);
        }
    });
    // We need a separate thread here because serenty will try to thread::sleep() and that
    // breaks tokio (which is used by the irc library.
    let irc_messages = std::thread::spawn(move || loop {
        let (ctx, msg) = for_discord
            .recv()
            .expect("failed to get message from IRC thread");
        ctx.channel_id
            .say(&ctx.http, msg)
            .expect("Failed to respond on discord");
    });
    let irc = std::thread::spawn(move || {
        let thread = async move {
            use futures::prelude::*;
            use irc::client::prelude::*;
            let config = Config {
                nickname: Some(irccfg.ircnick.to_owned()),
                nick_password: Some(irccfg.ircpass.to_owned()),
                server: Some("irc.speedrunslive.com".to_owned()),
                channels: vec!["#speedrunslive".to_owned()],
                port: Some(6667),
                use_tls: Some(false),
                ..irc::client::prelude::Config::default()
            };
            let mut client = Client::from_config(config)
                .await
                .expect("Failed to connect to SRL");
            client.identify().expect("Failed to identify with nickserv");
            println!("Connected to IRC as {}", client.current_nickname());
            // This substition will scrub all the control characters from the input
            // https://modern.ircdocs.horse/formatting.html
            // https://www.debuggex.com/r/5mzH8NGlLB6RyqaL
            let scrub = r"\x03([0-9]{1,2}(,[0-9]{1,2})?)?|\x04[a-fA-F0-9]{6}|[\x02\x0f\x11\x16\x1d\x1e\x1f]";
            let scrub_re = Regex::new(scrub).expect("Failed to build scrub");
            let mut start_race = StartRace::new(receiver, client.sender(), to_discord);

            let mut irc_stream = client.stream().expect("failed to make irc stream");
            loop {
                // First we check if discord has any work for us to do
                start_race.poll_discord();
                // Next we go through the IRC messages, and to do that
                // we do some normalization and cleanup just to make messages
                // easier to deal with.
                let irc_msg = irc_stream
                    .select_next_some()
                    .await
                    .expect("failed to get next message");
                let m = match irc_msg.clone().command {
                    Command::PRIVMSG(channel, message) => Some((channel, message)),
                    Command::NOTICE(channel, message) => Some((channel, message)),
                    _ => None,
                };
                if let Some((channel, message)) = m {
                    // Clean out color codes and what have you.
                    let message = scrub_re.replace_all(&message, "");

                    // At this point, we have a nicely normalized message to deal with
                    println!("channel = {}, message = '{}'", channel, message);

                    // Check to see if racebot has responded about any of our pending races
                    start_race.update_pending_races(&irc_msg, &message);

                    // Check if we should observe a newly created room
                    if irc_msg.source_nickname() == Some(RACEBOT) {
                        // Join any mega man hacks race room and just chill
                        let re = Regex::new(ROOM).expect("Failed to build regex");
                        println!("state: not waiting, message = '{}'", &message);
                        let chan = re.captures(&message).map(|c| c.name("chan"));
                        let game = re.captures(&message).map(|c| c.name("game"));
                        match chan.and_then(std::convert::identity) {
                            None => (),
                            Some(c) => {
                                let game = game
                                    .and_then(std::convert::identity)
                                    .map(|g| g.as_str())
                                    .unwrap_or("unknown");
                                for rc in RACECONFIGS {
                                    if rc.game_name == game {
                                        println!("joining {}", c.as_str());
                                        client
                                            .send_join(&c.as_str())
                                            .expect("Failed to join race channel");
                                        continue;
                                    }
                                }
                            }
                        };
                    }

                    // The rest of this loop is just processing individual commands from IRC
                    if message.starts_with(".entrants") {
                        let split = message.split_whitespace().collect::<Vec<_>>();
                        if split.len() > 1 {
                            let entrants: Entrants = srl_http::entrants(SRL_API, &split[1])
                                .await
                                .expect("Failed to get race entrants");
                            println!("{:#?}", entrants);
                        }
                    }
                    if message.starts_with(".botenter") || message.starts_with(".botjoin") {
                        let ch = irc_msg.response_target().unwrap_or(&channel);
                        client
                            .send_privmsg(&ch, ".enter")
                            .expect("Failed send_privmsg");
                    }
                    if message.starts_with(".botquit") || message.starts_with(".botforfeit") {
                        let ch = irc_msg.response_target().unwrap_or(&channel);
                        client
                            .send_privmsg(&ch, ".quit")
                            .expect("Failed send_privmsg");
                    }
                    if message.starts_with(".roll") {
                        use rand::Rng;
                        let ch = irc_msg.response_target().unwrap_or(&channel);
                        let seed: i32 = rand::thread_rng().gen_range(0, i32::max_value());
                        let alpha_seed = convert_to_base26(seed);
                        client
                            .send_privmsg(&ch, format!("Your seed is: {}", alpha_seed))
                            .expect("Failed send_privmesg");
                    }
                    if message.starts_with(".multi") {
                        let ch = irc_msg.response_target().unwrap_or(&channel);
                        let prefix = "#srl-";
                        if ch.starts_with(prefix) {
                            let (_, raceid) = ch.split_at(prefix.len());
                            let entrants: Entrants = srl_http::entrants(SRL_API, raceid)
                                .await
                                .expect("failed to get entrants");
                            let twitches = entrants
                                .entrants
                                .iter()
                                .map(|(_, v)| v.twitch.to_owned())
                                .filter(|n| n.len() > 0)
                                .collect::<Vec<_>>();
                            let mut url = "http://multitwitch.tv/".to_owned();
                            url.push_str(&twitches.join("/"));
                            if twitches.len() == 0 {
                                client
                                    .send_privmsg(&ch, "Sorry, no registered streams")
                                    .expect("Failed to send_privmsg");
                            } else {
                                client
                                    .send_privmsg(&ch, format!("Multitwitch URL: {}", &url))
                                    .expect("Failed to send_privmsg");
                            }
                        }
                    }
                    // Print a friendly message anytime someone mentions us
                    if message.contains(client.current_nickname()) {
                        let ch = irc_msg.response_target().unwrap_or(&channel);
                        client
                            .send_privmsg(&ch, "beep boop")
                            .expect("Failed send_privmsg");
                    }
                }
            }
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
    if msg.author.bot {
        return Ok(());
    } // Don't respond to bots
    let mut details = match msg.guild(&ctx.cache) {
        None => "unknown guild".to_owned(),
        Some(g) => format!("From guild with name '{}'", g.read().name),
    };
    details.push_str(format!("\nFrom user with display_name '{}'", msg.author.name).as_str());
    if let Some(cname) = msg.channel_id.name(&ctx.cache) {
        details.push_str(format!("\nFrom channel '{}'", cname).as_str());
    }
    msg.reply(ctx, format!("Ping received.\n{}", details))?;
    Ok(())
}

#[command]
fn roll(ctx: &mut Context, msg: &Message) -> CommandResult {
    use rand::Rng;
    if msg.author.bot {
        return Ok(());
    } // Don't respond to bots
    let seed: i32 = rand::thread_rng().gen_range(0, i32::max_value());
    let alpha_seed = convert_to_base26(seed);
    msg.reply(ctx, format!("Your seed is: {}", alpha_seed))?;
    Ok(())
}

#[command]
fn lottery(ctx: &mut Context, msg: &Message) -> CommandResult {
    use rand::distributions::uniform::Uniform;
    use rand::distributions::Distribution;

    if msg.author.bot {
        return Ok(());
    } // Don't respond to bots
    let tickets = msg.content.split_whitespace().skip(1).collect::<Vec<_>>();
    let winning_idx = if tickets.len() > 0 {
        let dist = Uniform::new_inclusive(0, tickets.len()-1);
        let mut rng = rand::thread_rng();
        Some(dist.sample(&mut rng))
    } else {
        None
    };

    match winning_idx {
        None      => msg.reply(ctx, "Invalid lottery")?,
        Some(idx) => msg.reply(ctx, format!("Winner is: {}", tickets[idx]))?,
    };
    Ok(())
}

#[command]
fn startrace(ctx: &mut Context, msg: &Message) -> CommandResult {
    if msg.author.bot {
        return Ok(());
    } // Don't respond to bots
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
                (*chan).lock().unwrap().send((new_ctx, msg.clone(), *rc))?;
                break;
            }
        }
    }
    Ok(())
}

fn convert_to_base26(seed: i32) -> String {
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

#[derive(Clone, Debug)]
enum RacebotStartRaceState {
    Waiting(ContextWrapper, Instant, RaceConfig),
    Timedout,
    Failed(ContextWrapper, Instant, RaceConfig, String), // error message from racebot
    Bad,
    Success(String), // channel
}

impl RacebotStartRaceState {
    fn failed(self, msg: &str) -> RacebotStartRaceState {
        match self {
            RacebotStartRaceState::Waiting(ctx, when, config) => {
                RacebotStartRaceState::Failed(ctx, when, config, msg.to_owned())
            }
            _ => RacebotStartRaceState::Bad,
        }
    }
    fn get_context(&self) -> Option<ContextWrapper> {
        match self {
            RacebotStartRaceState::Waiting(ctx, _, _) => Some(ctx.clone()),
            RacebotStartRaceState::Failed(ctx, _, _, _) => Some(ctx.clone()),
            _ => None,
        }
    }
    fn get_config(&self) -> Option<RaceConfig> {
        match self {
            RacebotStartRaceState::Waiting(_, _, config) => Some(config.clone()),
            RacebotStartRaceState::Failed(_, _, config, _) => Some(config.clone()),
            _ => None,
        }
    }

    fn check_racebot_response(self, source_nickname: &Option<&str>, message: &str) -> Self {
        if *source_nickname != Some(RACEBOT) {
            return self;
        }
        if message == "You've already started a race. Please use .setgame in the race channel if you need to set it to the correct game."{
            return self.failed(&message);
        }
        let re = Regex::new(ROOM).expect("Failed to build regex");
        let chan = re.captures(&message).map(|c| c.name("chan"));
        let game = re.captures(&message).map(|c| c.name("game"));
        let c = chan.and_then(std::convert::identity).unwrap();

        match self {
            RacebotStartRaceState::Waiting(_, sent_at, config) => {
                if sent_at.elapsed() >= Duration::from_secs(RACEBOTWAIT) {
                    println!("channel creation event arrived late");
                    return RacebotStartRaceState::Timedout;
                }
                let chan = c.as_str();
                let game = game
                    .and_then(std::convert::identity)
                    .map(|g| g.as_str())
                    .unwrap_or("unknown");
                println!("chan is '{}'", chan);
                println!("game is '{}'", game);
                if game == config.game_name {
                    return RacebotStartRaceState::Success(chan.to_string());
                } else {
                    // ignore the message from racebot, it must have been
                    // for someone else
                    return self;
                }
            }
            _ => return RacebotStartRaceState::Bad,
        }
    }
}

#[derive(Debug)]
struct StartRace {
    pending_races: VecDeque<RacebotStartRaceState>,
    receiver: std::sync::mpsc::Receiver<(ContextWrapper, Message, RaceConfig)>,
    send_client: irc::client::Sender,
    to_discord: std::sync::mpsc::Sender<(ContextWrapper, String)>,
}

impl StartRace {
    fn new(
        receiver: std::sync::mpsc::Receiver<(ContextWrapper, Message, RaceConfig)>,
        send_client: irc::client::Sender,
        to_discord: std::sync::mpsc::Sender<(ContextWrapper, String)>,
    ) -> Self {
        StartRace {
            pending_races: VecDeque::new(),
            receiver: receiver,
            send_client: send_client,
            to_discord: to_discord,
        }
    }

    fn poll_discord(&mut self) {
        use irc::client::prelude::*;
        match self.receiver.try_recv() {
            Err(_) => { /* TODO: should we exit or what? */ }
            Ok((ctx, _msg, config)) => {
                println!("starting race");
                self.pending_races.push_back(RacebotStartRaceState::Waiting(
                    ctx.clone(),
                    Instant::now(),
                    config,
                ));
                self.send_client
                    .send(Command::PRIVMSG(
                        SRL.to_string(),
                        format!(".startrace {}", config.game_code),
                    ))
                    .expect("Failed to startrace");
            }
        }
    }

    fn update_pending_races(&mut self, irc_msg: &irc::client::prelude::Message, message: &str) {
        let mut new_pending_races = VecDeque::new();
        while let Some(r) = self.pending_races.pop_front() {
            use RacebotStartRaceState::*;
            let ctx = r.get_context();
            let config = r.get_config();
            let new_r = r.check_racebot_response(&irc_msg.source_nickname(), &message);
            match new_r {
                Waiting(_, _, _) => new_pending_races.push_back(new_r),
                Success(chan) => {
                    if let Some(ctx) = ctx {
                        self.to_discord
                            .send((ctx, format!("/join {}", &chan)))
                            .expect("failed to send to irc_message thread");
                    }
                    println!("joining {}", &chan);
                    self.send_client
                        .send_join(&chan)
                        .expect("Failed to join race channel");
                    println!("setting goal");
                    if let Some(config) = config {
                        self.send_client
                            .send_privmsg(&chan, format!(".setgoal {}", config.race_goal))
                            .expect("Failed to setgoal");
                    }
                    self.send_client
                        .send_privmsg(&chan, ".enter")
                        .expect("Failed to enter");
                }
                Failed(ctx, _, _, message) => {
                    self.to_discord
                        .send((ctx, message.to_string()))
                        .expect("Failed to send to discord");
                }
                Timedout => {
                    println!("Letting discord know we failed to get a channel in time");
                    if let Some(ctx) = ctx {
                        self.to_discord.send((ctx, "Sorry, something went wrong. Check IRC, you might have a channel waiting.".to_string()))
                            .expect("Failed to respond on discord");
                    }
                }
                Bad => {
                    println!("racebot waiting entered illegal state");
                }
            }
        }
        self.pending_races = new_pending_races;
    }
}
