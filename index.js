const Discord = require('discord.js');
const client = new Discord.Client();
const config = require('./config.json')

client.on('ready', () => {
  console.log(`Logged in as ${client.user.tag}!`);
});

client.on('message', msg => {
  if (!msg.content.startsWith(config.prefix)) return;
  if (msg.content.startsWith(config.prefix + 'roll') || msg.author.bot) {
    const seed = Math.abs(Math.trunc(Math.random() * 0x7FFFFFFF));
    const alphaSeed = Convert10To26(seed);
    msg.reply('Your seed is: ' + alphaSeed);
  }
});

client.login(config.token);

function Convert10To26(x)
{
  var output = "";
  var newBase = ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z"];
  const base = newBase.length;
  for(var i = x; 0 < i; i = Math.floor(i / base)) {
    output += newBase[i % base];
  }
  // Reverse the string
  return output.split("").reverse().join("");
}
