"""The WebSocket transport: the server upgrades `/ws/users` and pushes a `User` message, whose
field it renames at S; the client (stream.py) reads it off each message.

Servers, each its stack's usual upgrade: gorilla/websocket in a gin handler, a NestJS
`@WebSocketGateway({ path })` on the ws adapter, `ws`'s `WebSocketServer({ server, path })` beside
Express, Spring's `WebSocketConfigurer` + `TextWebSocketHandler`, FastAPI's `@router.websocket`,
axum's `WebSocketUpgrade`, an ASP.NET Core controller action accepting the upgrade. The path is
built the `wiring_source` way: `/ws/users`, a `WS_PREFIX` constant + `/users`, or `/ws/` + the
`RouteKey` enum's `users` (no enum on Spring, axum or ASP.NET: gen.py's table)."""

from . import channel, stream
from .channel import pick
from .common import fill
from .rest_servers import ADA, ADA_EMAIL

CATEGORY = "WS_CONNECTS"
STATES = channel.STATES
# A generated SDK is no WebSocket idiom: the stacks here write their socket clients by hand.
UNEXPRESSIBLE = [
    (("transport", "websocket"), ("wiring_call", "generated-sdk"),
     "WebSocket clients are hand-written; no generator writes a socket SDK for these stacks"),
]
EXCHANGE = {"key": stream.WS_PATH, "path": stream.WS_PATH, "match": [r"/ws/users\b"]}


def go_gin(ax, f):
    ws = ax["wiring_source"]
    files = {
        "go.mod": channel.go_mod([("github.com/gin-gonic/gin", "v1.10.0"),
                                  ("github.com/gorilla/websocket", "v1.5.3")]),
        "main.go": fill(
            """
            package main

            import "github.com/gin-gonic/gin"

            func main() {
            \tr := gin.Default()
            \tr.GET(<<path>>, userStream)
            \tif err := r.Run(":8080"); err != nil {
            \t\tpanic(err)
            \t}
            }
            """,
            path=pick(ws, '"/ws/users"', 'wsPrefix + "/users"', '"/ws/" + string(RouteUsers)'),
        ),
        "stream.go": fill(
            """
            package main

            import (
            \t"net/http"

            \t"github.com/gin-gonic/gin"
            \t"github.com/gorilla/websocket"
            )

            // User is the user the stream pushes.
            type User struct {
            \tID    string `json:"id"`
            \t<<pascal>> string `json:"<<camel>>"`
            \tEmail string `json:"email"`
            }

            var upgrader = websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}

            func userStream(c *gin.Context) {
            \tconn, err := upgrader.Upgrade(c.Writer, c.Request, nil)
            \tif err != nil {
            \t\treturn
            \t}
            \tdefer conn.Close()
            \t_ = conn.WriteJSON(User{ID: "1", <<pascal>>: "<<ada>>", Email: "<<mail>>"})
            }
            """,
            pascal=f.pascal, camel=f.camel, ada=ADA, mail=ADA_EMAIL,
        ),
    }
    if ws == "const":
        files["routes.go"] = 'package main\n\nconst wsPrefix = "/ws"\n'
    elif ws == "enum-prefix":
        files["routes.go"] = ("package main\n\n// RouteKey names a resource's route segment.\n"
                              'type RouteKey string\n\nconst RouteUsers RouteKey = "users"\n')
    return files, {"route_file": "main.go", "handler": "userStream", "handler_file": "stream.go",
                   "model_file": "stream.go", "pair_files": ["main.go", "stream.go"],
                   "eco": "go"}


def _ts_routes(ws):
    return pick(ws, None, "export const WS_PREFIX = '/ws';\n",
                "export enum RouteKey {\n  Users = 'users',\n}\n")


def ts_nest(ax, f):
    ws = ax["wiring_source"]
    files = channel.node({"@nestjs/common": "^10.4.0", "@nestjs/core": "^10.4.0",
                          "@nestjs/platform-ws": "^10.4.0", "@nestjs/websockets": "^10.4.0",
                          "reflect-metadata": "^0.2.2", "rxjs": "^7.8.1", "ws": "^8.18.0"},
                         {"@types/ws": "^8.5.12"})
    gw = "src/users/users.gateway.ts"
    files.update({
        "src/main.ts": fill(
            """
            import { NestFactory } from '@nestjs/core';
            import { WsAdapter } from '@nestjs/platform-ws';
            import { AppModule } from './app.module';

            async function bootstrap() {
              const app = await NestFactory.create(AppModule);
              app.useWebSocketAdapter(new WsAdapter(app));
              await app.listen(3000);
            }

            void bootstrap();
            """
        ),
        "src/app.module.ts": fill(
            """
            import { Module } from '@nestjs/common';
            import { UsersGateway } from './users/users.gateway';

            @Module({ providers: [UsersGateway] })
            export class AppModule {}
            """
        ),
        "src/users/user.dto.ts": fill(
            """
            export class UserDto {
              id!: string;
              <<f>>!: string;
              email!: string;
            }
            """,
            f=f.camel,
        ),
        gw: fill(
            """
            import { OnGatewayConnection, WebSocketGateway } from '@nestjs/websockets';
            import type { WebSocket } from 'ws';
            <<imp>>import { UserDto } from './user.dto';

            @WebSocketGateway({ path: <<path>> })
            export class UsersGateway implements OnGatewayConnection {
              handleConnection(client: WebSocket): void {
                const user: UserDto = { id: '1', <<f>>: '<<ada>>', email: '<<mail>>' };
                client.send(JSON.stringify(user));
              }
            }
            """,
            imp=pick(ws, "", "import { WS_PREFIX } from '../routes';\n",
                     "import { RouteKey } from '../routes';\n"),
            path=pick(ws, "'/ws/users'", "`${WS_PREFIX}/users`", "`/ws/${RouteKey.Users}`"),
            f=f.camel, ada=ADA, mail=ADA_EMAIL,
        ),
    })
    if ws != "literal":
        files["src/routes.ts"] = _ts_routes(ws)
    return files, {"route_file": gw, "handler": "handleConnection", "handler_file": gw,
                   "model_file": "src/users/user.dto.ts", "pair_files": [gw], "eco": "node"}


def ts_express(ax, f):
    ws = ax["wiring_source"]
    files = channel.node({"express": "^4.21.0", "ws": "^8.18.0"},
                         {"@types/express": "^4.17.21", "@types/ws": "^8.5.12"})
    files.update({
        "src/app.ts": fill(
            """
            import { createServer } from 'http';
            import express from 'express';
            import { WebSocketServer } from 'ws';
            <<imp>>import { sendUser } from './stream';

            const app = express();
            const server = createServer(app);
            const wss = new WebSocketServer({ server, path: <<path>> });
            wss.on('connection', sendUser);
            server.listen(3000);
            """,
            imp=pick(ws, "", "import { WS_PREFIX } from './routes';\n",
                     "import { RouteKey } from './routes';\n"),
            path=pick(ws, "'/ws/users'", "`${WS_PREFIX}/users`", "`/ws/${RouteKey.Users}`"),
        ),
        "src/stream.ts": fill(
            """
            import type { WebSocket } from 'ws';

            export interface User {
              id: string;
              <<f>>: string;
              email: string;
            }

            export function sendUser(socket: WebSocket): void {
              const user: User = { id: '1', <<f>>: '<<ada>>', email: '<<mail>>' };
              socket.send(JSON.stringify(user));
            }
            """,
            f=f.camel, ada=ADA, mail=ADA_EMAIL,
        ),
    })
    if ws != "literal":
        files["src/routes.ts"] = _ts_routes(ws)
    return files, {"route_file": "src/app.ts", "handler": "sendUser",
                   "handler_file": "src/stream.ts", "model_file": "src/stream.ts",
                   "pair_files": ["src/app.ts", "src/stream.ts"], "eco": "node"}


JAVA = "src/main/java/com/sibyl/api/"


def java_spring(ax, f):
    ws = ax["wiring_source"]
    files = {
        "build.gradle.kts": channel.gradle_spring(
            ["org.springframework.boot:spring-boot-starter-websocket"]),
        JAVA + "Application.java": fill(
            """
            package com.sibyl.api;

            import org.springframework.boot.SpringApplication;
            import org.springframework.boot.autoconfigure.SpringBootApplication;

            @SpringBootApplication
            public class Application {
                public static void main(String[] args) {
                    SpringApplication.run(Application.class, args);
                }
            }
            """
        ),
        JAVA + "WebSocketConfig.java": fill(
            """
            package com.sibyl.api;

            import org.springframework.context.annotation.Configuration;
            import org.springframework.web.socket.config.annotation.EnableWebSocket;
            import org.springframework.web.socket.config.annotation.WebSocketConfigurer;
            import org.springframework.web.socket.config.annotation.WebSocketHandlerRegistry;

            @Configuration
            @EnableWebSocket
            public class WebSocketConfig implements WebSocketConfigurer {
                private final UserStreamHandler handler;

                public WebSocketConfig(UserStreamHandler handler) {
                    this.handler = handler;
                }

                @Override
                public void registerWebSocketHandlers(WebSocketHandlerRegistry registry) {
                    registry.addHandler(handler, <<path>>);
                }
            }
            """,
            path=pick(ws, '"/ws/users"', 'ApiPaths.WS_PREFIX + "/users"'),
        ),
        JAVA + "UserStreamHandler.java": fill(
            """
            package com.sibyl.api;

            import com.fasterxml.jackson.databind.ObjectMapper;
            import org.springframework.stereotype.Component;
            import org.springframework.web.socket.TextMessage;
            import org.springframework.web.socket.WebSocketSession;
            import org.springframework.web.socket.handler.TextWebSocketHandler;

            @Component
            public class UserStreamHandler extends TextWebSocketHandler {
                private final ObjectMapper mapper = new ObjectMapper();

                @Override
                public void afterConnectionEstablished(WebSocketSession session) throws Exception {
                    UserDto user = new UserDto("1", "<<ada>>", "<<mail>>");
                    session.sendMessage(new TextMessage(mapper.writeValueAsString(user)));
                }
            }
            """,
            ada=ADA, mail=ADA_EMAIL,
        ),
        JAVA + "UserDto.java": f"package com.sibyl.api;\n\npublic record UserDto(String id, "
                               f"String {f.camel}, String email) {{}}\n",
    }
    if ws == "const":
        files[JAVA + "ApiPaths.java"] = fill(
            """
            package com.sibyl.api;

            public final class ApiPaths {
                public static final String WS_PREFIX = "/ws";

                private ApiPaths() {}
            }
            """
        )
    cfg, h = JAVA + "WebSocketConfig.java", JAVA + "UserStreamHandler.java"
    return files, {"route_file": cfg, "handler": "afterConnectionEstablished", "handler_file": h,
                   "model_file": JAVA + "UserDto.java", "pair_files": [cfg, h],
                   "eco": "gradle", "settings": channel.GRADLE_SETTINGS}


def python_fastapi(ax, f):
    ws = ax["wiring_source"]
    files = {
        "pyproject.toml": channel.pyproject(["fastapi>=0.115", "pydantic>=2.8"]),
        "app/__init__.py": "",
        "app/main.py": "from fastapi import FastAPI\n\nfrom . import stream\n\napp = FastAPI()\n"
                       "app.include_router(stream.router)\n",
        "app/models.py": fill(
            """
            from pydantic import BaseModel


            class User(BaseModel):
                id: str
                <<f>>: str
                email: str
            """,
            f=f.camel,
        ),
        "app/stream.py": fill(
            """
            from fastapi import APIRouter, WebSocket

            <<imp>>from .models import User

            router = APIRouter()


            @router.websocket(<<path>>)
            async def user_stream(websocket: WebSocket) -> None:
                await websocket.accept()
                user = User(id="1", <<f>>="<<ada>>", email="<<mail>>")
                await websocket.send_json(user.model_dump())
                await websocket.close()
            """,
            imp=pick(ws, "", "from .routes import WS_PREFIX\n", "from .routes import RouteKey\n"),
            path=pick(ws, '"/ws/users"', 'f"{WS_PREFIX}/users"', 'f"/ws/{RouteKey.USERS}"'),
            f=f.camel, ada=ADA, mail=ADA_EMAIL,
        ),
    }
    if ws == "const":
        files["app/routes.py"] = 'WS_PREFIX = "/ws"\n'
    elif ws == "enum-prefix":
        files["app/routes.py"] = ('from enum import StrEnum\n\n\nclass RouteKey(StrEnum):\n'
                                  '    USERS = "users"\n')
    return files, {"route_file": "app/stream.py", "handler": "user_stream",
                   "handler_file": "app/stream.py", "model_file": "app/models.py",
                   "pair_files": ["app/stream.py"], "eco": "python"}


def rust_axum(ax, f):
    ws = ax["wiring_source"]
    router = pick(ws, 'Router::new().route("/ws/users", get(stream::user_stream))',
                  "Router::new().nest(\n        WS_PREFIX,\n"
                  '        Router::new().route("/users", get(stream::user_stream)),\n    )')
    files = {
        "Cargo.toml": channel.cargo([("axum", '{ version = "0.8", features = ["ws"] }'),
                                     ("serde", '{ version = "1", features = ["derive"] }'),
                                     ("serde_json", '"1"'),
                                     ("tokio", '{ version = "1", features = ["full"] }')]),
        "src/main.rs": fill(
            """
            use axum::{routing::get, Router};

            mod stream;
            <<konst>>
            #[tokio::main]
            async fn main() {
                let app = <<router>>;
                let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
                axum::serve(listener, app).await.unwrap();
            }
            """,
            konst=pick(ws, "", '\nconst WS_PREFIX: &str = "/ws";\n'), router=router,
        ),
        "src/stream.rs": fill(
            """
            use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
            use axum::response::Response;
            use serde::Serialize;

            #[derive(Serialize)]
            #[serde(rename_all = "camelCase")]
            pub struct User {
                pub id: String,
                pub <<snake>>: String,
                pub email: String,
            }

            pub async fn user_stream(ws: WebSocketUpgrade) -> Response {
                ws.on_upgrade(send_user)
            }

            async fn send_user(mut socket: WebSocket) {
                let user = User {
                    id: "1".to_string(),
                    <<snake>>: "<<ada>>".to_string(),
                    email: "<<mail>>".to_string(),
                };
                if let Ok(text) = serde_json::to_string(&user) {
                    let _ = socket.send(Message::Text(text.into())).await;
                }
            }
            """,
            snake=f.snake, ada=ADA, mail=ADA_EMAIL,
        ),
    }
    return files, {"route_file": "src/main.rs", "handler": "user_stream",
                   "handler_file": "src/stream.rs", "model_file": "src/stream.rs",
                   "pair_files": ["src/main.rs", "src/stream.rs"], "eco": "cargo"}


def csharp_aspnet(ax, f):
    ws = ax["wiring_source"]
    ctl = "Controllers/UserStreamController.cs"
    files = {
        "Api.csproj": channel.csproj([]),
        "Program.cs": fill(
            """
            var builder = WebApplication.CreateBuilder(args);
            builder.Services.AddControllers();
            var app = builder.Build();
            app.UseWebSockets();
            app.MapControllers();
            app.Run();
            """
        ),
        ctl: fill(
            """
            using System.Net.WebSockets;
            using System.Text.Json;
            using Microsoft.AspNetCore.Mvc;
            using Sibyl.Api.Models;

            namespace Sibyl.Api.Controllers;

            public class UserStreamController : ControllerBase
            {
                [Route(<<route>>)]
                public async Task Get()
                {
                    if (!HttpContext.WebSockets.IsWebSocketRequest)
                    {
                        HttpContext.Response.StatusCode = StatusCodes.Status400BadRequest;
                        return;
                    }
                    using var socket = await HttpContext.WebSockets.AcceptWebSocketAsync();
                    var user = new UserDto("1", "<<ada>>", "<<mail>>");
                    var bytes = JsonSerializer.SerializeToUtf8Bytes(user, new JsonSerializerOptions(JsonSerializerDefaults.Web));
                    await socket.SendAsync(bytes, WebSocketMessageType.Text, true, CancellationToken.None);
                }
            }
            """,
            route=pick(ws, '"/ws/users"', 'ApiPaths.WsPrefix + "/users"'), ada=ADA, mail=ADA_EMAIL,
        ),
        "Models/UserDto.cs": f"namespace Sibyl.Api.Models;\n\npublic record UserDto(string Id, "
                             f"string {f.pascal}, string Email);\n",
    }
    if ws == "const":
        files["ApiPaths.cs"] = fill(
            """
            namespace Sibyl.Api;

            public static class ApiPaths
            {
                public const string WsPrefix = "/ws";
            }
            """
        )
    return files, {"route_file": ctl, "handler": "Get", "handler_file": ctl,
                   "model_file": "Models/UserDto.cs", "pair_files": [ctl], "eco": "dotnet"}


SERVERS = {"go-gin": go_gin, "ts-nest": ts_nest, "ts-express": ts_express,
           "java-spring": java_spring, "python-fastapi": python_fastapi,
           "rust-axum": rust_axum, "csharp-aspnet": csharp_aspnet}
_client = stream.client("websocket")


def _server(ax, f):
    return SERVERS[ax["server"]](ax, f)


def render(case, state):
    return channel.assemble(case["axes"], state, _client, _server)[0]


def expected(case):
    return channel.expected(case, "websocket", CATEGORY, EXCHANGE, _client, _server,
                            "the user stream's message")
