import axios from 'axios';
import { Message } from '../models/message';

export async function getMessage(id: string): Promise<Message> {
  const res = await axios.get<Message>(`/api/messages/${id}`);
  return res.data;
}
