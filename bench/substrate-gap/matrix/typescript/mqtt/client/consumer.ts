import mqtt from 'mqtt';

const client = mqtt.connect('mqtt://localhost:1883');

client.on('connect', () => {
  client.subscribe('sensors/temp');
});

client.on('message', (topic: string, payload: Buffer) => {
  console.log(topic, payload.toString());
});
