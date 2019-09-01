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
    commands: [ping, irc, roll],
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

static RACEBOT: &str = "dagit";
//static SRL:     &str = "#speedrunslive";
static SRL: &str = "dagit";
// Race initiated for Mega Man Hacks. Join #srl-dr0nu to participate.
static ROOM: &str = r"Race initiated for Mega Man Hacks\. Join (?P<chan>\#srl\-[[:alnum:]]+) to participate\.";

fn main() {
    let config_reader = BufReader::new(File::open("config.json").expect("Failed opening file"));
    let config: Config = serde_json::from_reader(config_reader).expect("Failed decoding config");
    let irccfg = config.clone();

    let (sender, receiver) = channel(10);

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
        use irc::client::Client;
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

        let (sender, rec) = std::sync::mpsc::channel();
        let tsender = sender.clone();
        let godot1 = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)); // Not currently waiting on godot
        let godot2 = godot1.clone();

        reactor.register_client_with_handler(client, move |client, irc_msg| {
            print!("{}", irc_msg);
            if let Command::PRIVMSG(channel, message) = irc_msg.clone().command {
                // Check to see if we should note down the channel
                if godot1.load(std::sync::atomic::Ordering::SeqCst) {
                    let re = Regex::new(ROOM).expect("Failed to build regex");
                    let chan = re.captures(&message)
                        .map(|c| c.name("chan"));
                    match chan.and_then(std::convert::identity) {
                        None => (),
                        Some(c) => {
                            godot1.store(false, std::sync::atomic::Ordering::SeqCst);
                            println!("chan is '{}'", c.as_str());
                            match sender.send(Ok(c.as_str().to_owned())) {
                                Ok(()) => {},
                                Err(_) => {},
                            }
                        },
                    };
                }
                if message.contains(client.current_nickname()) {
                    let ch = irc_msg.response_target().unwrap_or(&channel);
                    client.send_privmsg(&ch, "beep boop").expect("Failed send_privmsg");
                    let chanlist = client.list_channels().unwrap_or(vec![])
                        .join(" ");
                    client.send_privmsg(&ch, format!("Currently in: {}", &chanlist)).expect("Failed send_privmsg");
                }
            }
            Ok(())
        });
        use irc::error::IrcError;
        use failure::Error;
        // This almost works but not quite.
        // The problem is that the threads spawns/joins happen immediately.
        // So by the time the first send_privmesg happens the timeout has already happened.
        // The actions like spawn/sleep/etc all need to be Futures as well instead of
        // actions that run now.
        // tokio future::lazy is probably what I want.
        // spawn might also be useful, same with oneshot channels.
        // Basically go read the tokio docs before trying this again.
        reactor.register_future(receiver.map_err(|e| {
            IrcError::Custom{ inner: Error::from_boxed_compat(Box::new(e)) }
        }).for_each(move |(ctx,_msg)| {
            send_client.send_privmsg(SRL, ".startrace megamanhacks").and_then(|()| {
                godot2.store(true, std::sync::atomic::Ordering::SeqCst);
                println!("now waiting for godot");
                let tsender = tsender.clone();
                let godot2 = godot2.clone();
                std::thread::spawn(move || {
                    struct TimeoutError;
                    std::thread::sleep(std::time::Duration::from_secs(30));
                    match tsender.send(Err(TimeoutError)) {
                        Ok(()) => {},
                        Err(_) => {},
                    };
                    godot2.store(false, std::sync::atomic::Ordering::SeqCst);
                println!("done waiting for godot: timeout");
                });
                match rec.recv() {
                    Ok(Ok(chan)) => {
                        send_client.send_join(&chan)?;
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        send_client.send_privmsg(&chan, ".setgoal mega man 2 randomizer - any% (easy)")?;
                        ctx.channel_id.say(&ctx.http, format!("/join {}", chan)).map_err(|e| {
                            IrcError::Custom{ inner: Error::from_boxed_compat(Box::new(e))}
                        })?;
                    },
                    _ => {},
                };
                Ok(())
            })
        }));
        reactor.run().unwrap();
    });
    discord.join().expect("Discord thread exited");
    irc.join().expect("irc thread exited");
}

#[command]
fn ping(ctx: &mut Context, msg: &Message) -> CommandResult {
    if msg.author.bot { return Ok(()) } // Don't respond to bots
    msg.reply(ctx, "Pong!")?;
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
fn irc(ctx: &mut Context, msg: &Message) -> CommandResult {
    if msg.author.bot { return Ok(()) } // Don't respond to bots
    // send a message to IRC
    let mut data = ctx.data.write();
    let chan = data.get_mut::<Chan>().unwrap();
    let new_ctx = ContextWrapper {
        http: ctx.http.clone(),
        channel_id: msg.channel_id,
    };
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
