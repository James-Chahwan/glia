import axios from 'axios';
import { Comment } from '../models/ticket';

export async function comments(ticketId: string): Promise<Comment[]> {
  const res = await axios.get<Comment[]>(`/api/tickets/${ticketId}/comments`);
  return res.data;
}

export async function addComment(ticketId: string, body: string): Promise<Comment> {
  const res = await fetch(`/api/tickets/${ticketId}/comments`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ body }),
  });
  return (await res.json()) as Comment;
}
