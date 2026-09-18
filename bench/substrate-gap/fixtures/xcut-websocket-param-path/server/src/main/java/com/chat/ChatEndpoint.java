package com.chat;

import jakarta.websocket.OnMessage;
import jakarta.websocket.Session;
import jakarta.websocket.server.ServerEndpoint;

@ServerEndpoint("/chat/{room}")
public class ChatEndpoint {
    @OnMessage
    public void onMessage(Session session, String message) {
    }
}
