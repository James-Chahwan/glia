"""The queue transport: the server publishes a `users.updated` event carrying the `User`, whose
field it renames at S; the client (stream.py) consumes the topic and reads the field. The broker
is Kafka, the one every stack of the matrix writes idiomatically.

Producers: segmentio/kafka-go (Go), NestJS `ClientKafka.emit`, kafkajs (Express's stack), Spring
`KafkaTemplate.send`, aiokafka (FastAPI's stack), rdkafka's `FutureProducer` (axum's stack),
Confluent.Kafka's `IProducer.ProduceAsync` (ASP.NET Core). The topic is built the
`wiring_source` way: the literal, a `USER_UPDATED_TOPIC` constant, or the `TopicKey` enum's
`users` + `.updated`."""

from . import channel, stream
from .channel import pick
from .common import fill
from .rest_servers import ADA, ADA_EMAIL

CATEGORY = "QUEUE_FLOWS"
STATES = channel.STATES
TOPIC = stream.TOPIC
UNEXPRESSIBLE = [
    (("transport", "queue"), ("wiring_call", "generated-sdk"),
     "Kafka consumers are hand-written; what a generator writes for a topic is its message "
     "model (contract=generated), not the subscription"),
]
EXCHANGE = {"key": TOPIC, "topic": TOPIC, "broker": "kafka", "match": [r"users\.updated"]}


def go_gin(ax, f):
    ws = ax["wiring_source"]
    files = {
        "go.mod": channel.go_mod([("github.com/segmentio/kafka-go", "v0.4.47")]),
        "main.go": fill(
            """
            package main

            import "context"

            func main() {
            \tif err := PublishUserUpdated(context.Background(), User{ID: "1", <<pascal>>: "<<ada>>", Email: "<<mail>>"}); err != nil {
            \t\tpanic(err)
            \t}
            }
            """,
            pascal=f.pascal, ada=ADA, mail=ADA_EMAIL,
        ),
        "events.go": fill(
            """
            package main

            import (
            \t"context"
            \t"encoding/json"

            \t"github.com/segmentio/kafka-go"
            )

            // User is the payload of a user-updated event.
            type User struct {
            \tID    string `json:"id"`
            \t<<pascal>> string `json:"<<camel>>"`
            \tEmail string `json:"email"`
            }

            var writer = &kafka.Writer{Addr: kafka.TCP("localhost:9092"), Topic: <<topic>>}

            // PublishUserUpdated publishes a user-updated event.
            func PublishUserUpdated(ctx context.Context, u User) error {
            \tb, err := json.Marshal(u)
            \tif err != nil {
            \t\treturn err
            \t}
            \treturn writer.WriteMessages(ctx, kafka.Message{Key: []byte(u.ID), Value: b})
            }
            """,
            pascal=f.pascal, camel=f.camel,
            topic=pick(ws, f'"{TOPIC}"', "UserUpdatedTopic", 'string(TopicUsers) + ".updated"'),
        ),
    }
    if ws == "const":
        files["topics.go"] = f'package main\n\nconst UserUpdatedTopic = "{TOPIC}"\n'
    elif ws == "enum-prefix":
        files["topics.go"] = ("package main\n\n// TopicKey names a topic's resource segment.\n"
                              'type TopicKey string\n\nconst TopicUsers TopicKey = "users"\n')
    return files, {"route_file": "events.go", "handler": "PublishUserUpdated",
                   "handler_file": "events.go", "model_file": "events.go",
                   "pair_files": ["events.go"], "eco": "go"}


def _ts_topics(ws):
    return pick(ws, None, f"export const USER_UPDATED_TOPIC = '{TOPIC}';\n",
                "export enum TopicKey {\n  Users = 'users',\n}\n")


def _ts_topic(ws):
    return pick(ws, f"'{TOPIC}'", "USER_UPDATED_TOPIC", "`${TopicKey.Users}.updated`")


def _ts_topic_import(ws, rel):
    return pick(ws, "", f"import {{ USER_UPDATED_TOPIC }} from '{rel}';\n",
                f"import {{ TopicKey }} from '{rel}';\n")


def ts_nest(ax, f):
    ws = ax["wiring_source"]
    files = channel.node({"@nestjs/common": "^10.4.0", "@nestjs/core": "^10.4.0",
                          "@nestjs/microservices": "^10.4.0", "kafkajs": "^2.2.4",
                          "reflect-metadata": "^0.2.2", "rxjs": "^7.8.1"})
    svc = "src/users/users.events.ts"
    files.update({
        "src/main.ts": fill(
            """
            import { NestFactory } from '@nestjs/core';
            import { AppModule } from './app.module';

            async function bootstrap() {
              const app = await NestFactory.create(AppModule);
              await app.listen(3000);
            }

            void bootstrap();
            """
        ),
        "src/app.module.ts": fill(
            """
            import { Module } from '@nestjs/common';
            import { ClientsModule, Transport } from '@nestjs/microservices';
            import { UserEvents } from './users/users.events';

            @Module({
              imports: [
                ClientsModule.register([
                  {
                    name: 'KAFKA',
                    transport: Transport.KAFKA,
                    options: { client: { brokers: ['localhost:9092'] } },
                  },
                ]),
              ],
              providers: [UserEvents],
            })
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
        svc: fill(
            """
            import { Inject, Injectable } from '@nestjs/common';
            import { ClientKafka } from '@nestjs/microservices';
            <<imp>>import { UserDto } from './user.dto';

            @Injectable()
            export class UserEvents {
              constructor(@Inject('KAFKA') private readonly kafka: ClientKafka) {}

              publishUpdated(id: string): void {
                const user: UserDto = { id, <<f>>: '<<ada>>', email: '<<mail>>' };
                this.kafka.emit(<<topic>>, user);
              }
            }
            """,
            imp=_ts_topic_import(ws, "../topics"), topic=_ts_topic(ws), f=f.camel, ada=ADA,
            mail=ADA_EMAIL,
        ),
    })
    if ws != "literal":
        files["src/topics.ts"] = _ts_topics(ws)
    return files, {"route_file": svc, "handler": "publishUpdated", "handler_file": svc,
                   "model_file": "src/users/user.dto.ts", "pair_files": [svc], "eco": "node"}


def ts_express(ax, f):
    ws = ax["wiring_source"]
    files = channel.node({"express": "^4.21.0", "kafkajs": "^2.2.4"},
                         {"@types/express": "^4.17.21"})
    files["src/events.ts"] = fill(
        """
        import { Kafka } from 'kafkajs';
        <<imp>>
        export interface User {
          id: string;
          <<f>>: string;
          email: string;
        }

        const kafka = new Kafka({ clientId: 'sibyl-api', brokers: ['localhost:9092'] });
        const producer = kafka.producer();

        export async function publishUserUpdated(id: string): Promise<void> {
          const user: User = { id, <<f>>: '<<ada>>', email: '<<mail>>' };
          await producer.connect();
          await producer.send({ topic: <<topic>>, messages: [{ key: user.id, value: JSON.stringify(user) }] });
        }
        """,
        imp=_ts_topic_import(ws, "./topics"), topic=_ts_topic(ws), f=f.camel, ada=ADA,
        mail=ADA_EMAIL,
    )
    if ws != "literal":
        files["src/topics.ts"] = _ts_topics(ws)
    return files, {"route_file": "src/events.ts", "handler": "publishUserUpdated",
                   "handler_file": "src/events.ts", "model_file": "src/events.ts",
                   "pair_files": ["src/events.ts"], "eco": "node"}


JAVA = "src/main/java/com/sibyl/api/"


def java_spring(ax, f):
    ws = ax["wiring_source"]
    ev = JAVA + "UserEvents.java"
    files = {
        "build.gradle.kts": channel.gradle_spring(["org.springframework.kafka:spring-kafka"]),
        "src/main/resources/application.yml": fill(
            """
            spring:
              kafka:
                bootstrap-servers: localhost:9092
                producer:
                  value-serializer: org.springframework.kafka.support.serializer.JsonSerializer
            """
        ),
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
        ev: fill(
            """
            package com.sibyl.api;

            import org.springframework.kafka.core.KafkaTemplate;
            import org.springframework.stereotype.Service;

            @Service
            public class UserEvents {
                private final KafkaTemplate<String, UserDto> kafka;

                public UserEvents(KafkaTemplate<String, UserDto> kafka) {
                    this.kafka = kafka;
                }

                public void publishUpdated(String id) {
                    kafka.send(<<topic>>, id, new UserDto(id, "<<ada>>", "<<mail>>"));
                }
            }
            """,
            topic=pick(ws, f'"{TOPIC}"', "Topics.USER_UPDATED"), ada=ADA, mail=ADA_EMAIL,
        ),
        JAVA + "UserDto.java": f"package com.sibyl.api;\n\npublic record UserDto(String id, "
                               f"String {f.camel}, String email) {{}}\n",
    }
    if ws == "const":
        files[JAVA + "Topics.java"] = fill(
            """
            package com.sibyl.api;

            public final class Topics {
                public static final String USER_UPDATED = "<<topic>>";

                private Topics() {}
            }
            """,
            topic=TOPIC,
        )
    return files, {"route_file": ev, "handler": "publishUpdated", "handler_file": ev,
                   "model_file": JAVA + "UserDto.java", "pair_files": [ev], "eco": "gradle",
                   "settings": channel.GRADLE_SETTINGS}


def python_fastapi(ax, f):
    ws = ax["wiring_source"]
    files = {
        "pyproject.toml": channel.pyproject(["fastapi>=0.115", "pydantic>=2.8", "aiokafka>=0.11"]),
        "app/__init__.py": "",
        "app/main.py": fill(
            """
            from contextlib import asynccontextmanager

            from fastapi import FastAPI

            from .events import producer


            @asynccontextmanager
            async def lifespan(app: FastAPI):
                await producer.start()
                yield
                await producer.stop()


            app = FastAPI(lifespan=lifespan)
            """
        ),
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
        "app/events.py": fill(
            """
            from aiokafka import AIOKafkaProducer

            <<imp>>from .models import User

            producer = AIOKafkaProducer(bootstrap_servers="localhost:9092")


            async def publish_user_updated(user_id: str) -> None:
                user = User(id=user_id, <<f>>="<<ada>>", email="<<mail>>")
                await producer.send_and_wait(<<topic>>, user.model_dump_json().encode())
            """,
            imp=pick(ws, "", "from .topics import USER_UPDATED_TOPIC\n",
                     "from .topics import TopicKey\n"),
            topic=pick(ws, f'"{TOPIC}"', "USER_UPDATED_TOPIC", 'f"{TopicKey.USERS}.updated"'),
            f=f.camel, ada=ADA, mail=ADA_EMAIL,
        ),
    }
    if ws == "const":
        files["app/topics.py"] = f'USER_UPDATED_TOPIC = "{TOPIC}"\n'
    elif ws == "enum-prefix":
        files["app/topics.py"] = ('from enum import StrEnum\n\n\nclass TopicKey(StrEnum):\n'
                                  '    USERS = "users"\n')
    return files, {"route_file": "app/events.py", "handler": "publish_user_updated",
                   "handler_file": "app/events.py", "model_file": "app/models.py",
                   "pair_files": ["app/events.py"], "eco": "python"}


def rust_axum(ax, f):
    ws = ax["wiring_source"]
    files = {
        "Cargo.toml": channel.cargo([("rdkafka", '"0.36"'),
                                     ("serde", '{ version = "1", features = ["derive"] }'),
                                     ("serde_json", '"1"'),
                                     ("tokio", '{ version = "1", features = ["full"] }')]),
        "src/main.rs": fill(
            """
            use rdkafka::config::ClientConfig;
            use rdkafka::producer::FutureProducer;

            mod events;

            #[tokio::main]
            async fn main() {
                let producer: FutureProducer = ClientConfig::new()
                    .set("bootstrap.servers", "localhost:9092")
                    .create()
                    .expect("kafka producer");
                if let Err(e) = events::publish_user_updated(&producer, "1").await {
                    eprintln!("publish failed: {e}");
                }
            }
            """
        ),
        "src/events.rs": fill(
            """
            use std::time::Duration;

            use rdkafka::error::KafkaError;
            use rdkafka::producer::{FutureProducer, FutureRecord};
            use serde::Serialize;
            <<konst>>
            #[derive(Serialize)]
            #[serde(rename_all = "camelCase")]
            pub struct User {
                pub id: String,
                pub <<snake>>: String,
                pub email: String,
            }

            pub async fn publish_user_updated(producer: &FutureProducer, id: &str) -> Result<(), KafkaError> {
                let user = User {
                    id: id.to_string(),
                    <<snake>>: "<<ada>>".to_string(),
                    email: "<<mail>>".to_string(),
                };
                let payload = serde_json::to_string(&user).unwrap_or_default();
                producer
                    .send(FutureRecord::to(<<topic>>).key(id).payload(&payload), Duration::from_secs(5))
                    .await
                    .map(|_| ())
                    .map_err(|(e, _)| e)
            }
            """,
            konst=pick(ws, "", f'\npub const USER_UPDATED_TOPIC: &str = "{TOPIC}";\n'),
            topic=pick(ws, f'"{TOPIC}"', "USER_UPDATED_TOPIC"), snake=f.snake, ada=ADA,
            mail=ADA_EMAIL,
        ),
    }
    return files, {"route_file": "src/events.rs", "handler": "publish_user_updated",
                   "handler_file": "src/events.rs", "model_file": "src/events.rs",
                   "pair_files": ["src/events.rs"], "eco": "cargo"}


def csharp_aspnet(ax, f):
    ws = ax["wiring_source"]
    ev = "Events/UserEvents.cs"
    files = {
        "Api.csproj": channel.csproj([("Confluent.Kafka", "2.5.3")]),
        "Program.cs": fill(
            """
            using Confluent.Kafka;
            using Sibyl.Api.Events;

            var builder = WebApplication.CreateBuilder(args);
            builder.Services.AddSingleton<IProducer<string, string>>(
                new ProducerBuilder<string, string>(new ProducerConfig { BootstrapServers = "localhost:9092" }).Build());
            builder.Services.AddSingleton<UserEvents>();
            var app = builder.Build();
            app.Run();
            """
        ),
        ev: fill(
            """
            using System.Text.Json;
            using Confluent.Kafka;
            using Sibyl.Api.Models;

            namespace Sibyl.Api.Events;

            public class UserEvents
            {
                private static readonly JsonSerializerOptions Web = new(JsonSerializerDefaults.Web);
                private readonly IProducer<string, string> _producer;

                public UserEvents(IProducer<string, string> producer) => _producer = producer;

                public Task PublishUpdatedAsync(string id)
                {
                    var user = new UserDto(id, "<<ada>>", "<<mail>>");
                    return _producer.ProduceAsync(<<topic>>,
                        new Message<string, string> { Key = id, Value = JsonSerializer.Serialize(user, Web) });
                }
            }
            """,
            topic=pick(ws, f'"{TOPIC}"', "Topics.UserUpdated"), ada=ADA, mail=ADA_EMAIL,
        ),
        "Models/UserDto.cs": f"namespace Sibyl.Api.Models;\n\npublic record UserDto(string Id, "
                             f"string {f.pascal}, string Email);\n",
    }
    if ws == "const":
        files["Topics.cs"] = fill(
            """
            namespace Sibyl.Api;

            public static class Topics
            {
                public const string UserUpdated = "<<topic>>";
            }
            """,
            topic=TOPIC,
        )
    return files, {"route_file": ev, "handler": "PublishUpdatedAsync", "handler_file": ev,
                   "model_file": "Models/UserDto.cs", "pair_files": [ev], "eco": "dotnet"}


SERVERS = {"go-gin": go_gin, "ts-nest": ts_nest, "ts-express": ts_express,
           "java-spring": java_spring, "python-fastapi": python_fastapi,
           "rust-axum": rust_axum, "csharp-aspnet": csharp_aspnet}
_client = stream.client("queue")


def _server(ax, f):
    return SERVERS[ax["server"]](ax, f)


def render(case, state):
    return channel.assemble(case["axes"], state, _client, _server)[0]


def expected(case):
    return channel.expected(case, "queue", CATEGORY, EXCHANGE, _client, _server,
                            "the users.updated event")
