use serenity::client::Client;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::prelude::{EventHandler, Context};
use serenity::framework::standard::{
    StandardFramework,
    CommandResult,
    macros::{
        command,
        group
    }
};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::BufReader;

#[derive(Debug, Serialize, Deserialize)]
struct Config {
    token: String,
    prefix: String,
}

group!({
    name: "general",
    options: {},
    commands: [ping, roll],
});


struct Handler;

impl EventHandler for Handler {
    fn ready(&self, _ctx: Context, bot: Ready)
    {
        println!("Logged in as {}", bot.user.tag());
    }
}

fn main() {
    let config_reader = BufReader::new(File::open("config.json").expect("Failed opening file"));
    let config: Config = serde_json::from_reader(config_reader).expect("Failed decoding config");

    let mut client = Client::new(&config.token, Handler)
        .expect("Error creating client");
    client.with_framework(StandardFramework::new()
        .configure(|c| c.prefix(&config.prefix))
        .group(&GENERAL_GROUP));

    // start listening for events by starting a single shard
    if let Err(why) = client.start() {
        println!("An error occured while running the client: {:?}", why);
    }

}

#[command]
fn ping(ctx: &mut Context, msg: &Message) -> CommandResult {
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
