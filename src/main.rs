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
    type Value = Sender<(ContextWrapper,Message)>;
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
static ROOM: &str = r"Race initiated for Mega Man Hacks\. Join (?P<chan>\#srl\-[[:alnum:]]+) to participate\.";
//static ROOM: &str = r"Race initiated.*Mega Man Hacks.*Join.*(?P<chan>\#srl\-[[:alnum:]]+).*";

fn main() {
    let config_reader = BufReader::new(File::open("config.json").expect("Failed opening file"));
    let config: Config = serde_json::from_reader(config_reader).expect("Failed decoding config");
    let irccfg = config.clone();

    let (sender, receiver) = channel(10);
    let (ctx_sender, ctx_receiver)
        :(std::sync::mpsc::Sender<(ContextWrapper,Instant)>
         ,std::sync::mpsc::Receiver<(ContextWrapper,Instant)>)
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
            nickname: Some(irccfg.ircnick),
            nick_password: Some(irccfg.ircpass),
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

        reactor.register_client_with_handler(client, move |client, irc_msg| {
            //print!("{}", irc_msg);
            if let Command::PRIVMSG(channel, message) = irc_msg.clone().command {
                // This substition will scrub all the control characters from the input
                // https://modern.ircdocs.horse/formatting.html
                // https://www.debuggex.com/r/5mzH8NGlLB6RyqaL
                let scrub = r"\x03([0-9]{1,2}(,[0-9]{1,2})?)?|\x04[a-fA-F0-9]{6}|[\x02\x0f\x11\x16\x1d\x1e\x1f]";
                let scrub_re = Regex::new(scrub).expect("Failed to build scub");
                let message = scrub_re.replace_all(&message, "");
                println!("message = '{}'", message);
                // Check to see if we should note down the channel
                if irc_msg.source_nickname() == Some(&RACEBOT) {
                    if godot1.load(std::sync::atomic::Ordering::SeqCst) {
                        let re = Regex::new(ROOM).expect("Failed to build regex");
                        println!("state: waiting on godot, message = '{}'", &message);
                        let chan = re.captures(&message)
                            .map(|c| c.name("chan"));
                        match chan.and_then(std::convert::identity) {
                            None => (),
                            Some(c) => {
                                godot1.store(false, std::sync::atomic::Ordering::SeqCst);
                                match ctx_receiver.try_iter().last() {
                                    Some((ctx,sent_at)) => {
                                        if sent_at.elapsed() < Duration::from_secs(RACEBOTWAIT) {
                                            let chan = c.as_str();
                                            println!("chan is '{}'", chan);
                                            println!("responding on discord");
                                            ctx.channel_id.say(&ctx.http, format!("/join {}", &chan)).map_err(|e| {
                                                IrcError::Custom{ inner: Error::from_boxed_compat(Box::new(e))}
                                            }).expect("Failed to respond on discord");
                                            println!("joining {}", &chan);
                                            client.send_join(&chan).expect("Failed to join race channel");
                                            println!("setting goal");
                                            client.send_privmsg(&chan, ".setgoal mega man 2 randomizer - any% (easy)").expect("Failed to setgoal");
                                            client.send_privmsg(&chan, ".enter").expect("Failed to setgoal");
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
                        match chan.and_then(std::convert::identity) {
                            None => (),
                            Some(c) => {
                                println!("joining {}", c.as_str());
                                client.send_join(&c.as_str()).expect("Failed to join race channel");
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
        }).for_each(move |(ctx,_msg)| {
            let godot2 = godot2.clone();
            let godot3 = godot2.clone();
            let send_client = send_client.clone();
            let ctx2 = ctx.clone();
            ctx_sender.send((ctx,Instant::now())).expect("Failed to send ctx");
            println!("set godot2 true");
            godot2.store(true, std::sync::atomic::Ordering::SeqCst);
            println!("starting race");
            send_client.send_privmsg(SRL, ".startrace megamanhacks").expect("Failed to startrace");
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
    msg.reply(&ctx, "Attempting to start race...")?;
    (*chan).try_send((new_ctx,msg.clone()))?;
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
