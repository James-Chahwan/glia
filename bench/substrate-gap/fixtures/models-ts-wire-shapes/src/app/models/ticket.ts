export interface Ticket {
  id: string;
  title: string;
  priority: number;
}

export interface NewTicket {
  title: string;
  priority: number;
}

export interface Comment {
  id: string;
  body: string;
}
