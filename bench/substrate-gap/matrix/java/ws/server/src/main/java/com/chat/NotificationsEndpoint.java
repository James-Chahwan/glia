package com.chat;

import jakarta.websocket.OnMessage;
import jakarta.websocket.Session;
import jakarta.websocket.server.ServerEndpoint;

@ServerEndpoint("/notifications")
public class NotificationsEndpoint {
    @OnMessage
    public void onMessage(Session session, String message) {
        session.getAsyncRemote().sendText(message);
    }
}
