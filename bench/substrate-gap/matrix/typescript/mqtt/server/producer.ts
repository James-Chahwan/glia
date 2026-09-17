import mqtt from 'mqtt';

const client = mqtt.connect('mqtt://localhost:1883');

export function publishReading(value: number): void {
  client.publish('sensors/temp', String(value));
}
