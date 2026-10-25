"""The gRPC transport: `users.v1.UserService/GetUser(GetUserRequest) returns (User)`, the
`User` message's `display_name` renamed at S (same field number: the wire keeps working, the code
reading the old accessor does not).

The server owns the .proto (`proto/users/v1/users.proto`; ASP.NET's `Protos/users.proto`) and
implements it the stack's way: grpc-go, NestJS `@GrpcMethod` over proto-loader, grpc-java's
`ImplBase` behind a Spring bean, tonic (with build.rs), Grpc.AspNetCore. Express and FastAPI
serve no gRPC (gen.py's table). The client holds the code protoc's plugin for its language wrote
(ts-proto, protoc_plugin, grpc-kotlin, grpc-swift + swift-protobuf, grpcio-tools), stale until it
regenerates at C, and builds the stub over a channel to the server's address, inline or through
a constant (`wiring_source`; no enum: a method path is no URL). `wrapper` puts a hand-written
`fetchUser` around the stub (`contract=declared-type` maps the message into the app's own
model), `generated-sdk` hands the app the stub itself. The pair is the stub's construction site
(glia reads no client site inside generated code) to the .proto's service."""

from . import channel, payload
from .channel import KT_API, app_import, block, konst, pick
from .common import fill
from .rest_clients import Out
from .rest_servers import ADA, ADA_EMAIL

CATEGORY = "GRPC_CALLS"
STATES = channel.STATES
SERVICE = "users.v1.UserService"
METHOD = "/users.v1.UserService/GetUser"
ADDR = "localhost:50051"
EXCHANGE = {"key": SERVICE, "method": METHOD, "match": [r"\bUserService\b"]}


def proto(f):
    return fill(
        """
        syntax = "proto3";

        package users.v1;

        option csharp_namespace = "Sibyl.Users.V1";
        option go_package = "example.com/sibyl/api/gen/users/v1;usersv1";
        option java_multiple_files = true;
        option java_package = "com.sibyl.users.v1";

        service UserService {
          rpc GetUser(GetUserRequest) returns (User);
        }

        message GetUserRequest {
          string id = 1;
        }

        message User {
          string id = 1;
          string <<snake>> = 2;
          string email = 3;
        }
        """,
        snake=f.snake,
    )


PROTO = "proto/users/v1/users.proto"


# ---------------------------------------------------------------------------------- servers

def go_gin(ax, f):
    ws = ax["wiring_source"]
    files = {
        "go.mod": channel.go_mod([("google.golang.org/grpc", "v1.67.1"),
                                  ("google.golang.org/protobuf", "v1.35.1")]),
        "buf.gen.yaml": "version: v2\nplugins:\n  - remote: buf.build/protocolbuffers/go\n"
                        "    out: gen\n    opt: paths=source_relative\n"
                        "  - remote: buf.build/grpc/go\n    out: gen\n"
                        "    opt: paths=source_relative\n",
        PROTO: proto(f),
        "main.go": fill(
            """
            package main

            import (
            \t"log"
            \t"net"

            \tusersv1 "example.com/sibyl/api/gen/users/v1"
            \t"google.golang.org/grpc"
            )

            func main() {
            \tlis, err := net.Listen("tcp", <<addr>>)
            \tif err != nil {
            \t\tlog.Fatal(err)
            \t}
            \ts := grpc.NewServer()
            \tusersv1.RegisterUserServiceServer(s, &userServer{})
            \tif err := s.Serve(lis); err != nil {
            \t\tlog.Fatal(err)
            \t}
            }
            """,
            addr=pick(ws, '":50051"', "grpcAddr"),
        ),
        "server.go": fill(
            """
            package main

            import (
            \t"context"

            \tusersv1 "example.com/sibyl/api/gen/users/v1"
            )

            type userServer struct {
            \tusersv1.UnimplementedUserServiceServer
            }

            func (s *userServer) GetUser(ctx context.Context, req *usersv1.GetUserRequest) (*usersv1.User, error) {
            \treturn &usersv1.User{Id: req.GetId(), <<pascal>>: "<<ada>>", Email: "<<mail>>"}, nil
            }
            """,
            pascal=f.pascal, ada=ADA, mail=ADA_EMAIL,
        ),
    }
    if ws == "const":
        files["config.go"] = 'package main\n\nconst grpcAddr = ":50051"\n'
    return files, {"route_file": PROTO, "handler": "GetUser", "handler_file": "server.go",
                   "model_file": PROTO, "pair_files": [PROTO, "server.go"], "eco": "go"}


def ts_nest(ax, f):
    ws = ax["wiring_source"]
    files = channel.node({"@grpc/grpc-js": "^1.12.2", "@grpc/proto-loader": "^0.7.13",
                          "@nestjs/common": "^10.4.0", "@nestjs/core": "^10.4.0",
                          "@nestjs/microservices": "^10.4.0", "reflect-metadata": "^0.2.2",
                          "rxjs": "^7.8.1"})
    ctl = "src/users/users.controller.ts"
    files.update({
        PROTO: proto(f),
        "src/main.ts": fill(
            """
            import { join } from 'path';
            import { NestFactory } from '@nestjs/core';
            import { MicroserviceOptions, Transport } from '@nestjs/microservices';
            import { AppModule } from './app.module';
            <<imp>>
            async function bootstrap() {
              const app = await NestFactory.createMicroservice<MicroserviceOptions>(AppModule, {
                transport: Transport.GRPC,
                options: {
                  package: 'users.v1',
                  protoPath: join(__dirname, '../proto/users/v1/users.proto'),
                  url: <<url>>,
                },
              });
              await app.listen();
            }

            void bootstrap();
            """,
            imp=pick(ws, "", "import { GRPC_URL } from './config';\n"),
            url=pick(ws, "'0.0.0.0:50051'", "GRPC_URL"),
        ),
        "src/app.module.ts": fill(
            """
            import { Module } from '@nestjs/common';
            import { UsersController } from './users/users.controller';

            @Module({ controllers: [UsersController] })
            export class AppModule {}
            """
        ),
        "src/users/users.interface.ts": fill(
            """
            export interface GetUserRequest {
              id: string;
            }

            export interface User {
              id: string;
              <<f>>: string;
              email: string;
            }
            """,
            f=f.camel,
        ),
        ctl: fill(
            """
            import { Controller } from '@nestjs/common';
            import { GrpcMethod } from '@nestjs/microservices';
            import type { GetUserRequest, User } from './users.interface';

            @Controller()
            export class UsersController {
              @GrpcMethod('UserService', 'GetUser')
              getUser(data: GetUserRequest): User {
                return { id: data.id, <<f>>: '<<ada>>', email: '<<mail>>' };
              }
            }
            """,
            f=f.camel, ada=ADA, mail=ADA_EMAIL,
        ),
    })
    if ws == "const":
        files["src/config.ts"] = "export const GRPC_URL = '0.0.0.0:50051';\n"
    return files, {"route_file": PROTO, "handler": "getUser", "handler_file": ctl,
                   "model_file": "src/users/users.interface.ts", "pair_files": [PROTO, ctl],
                   "eco": "node"}


JAVA = "src/main/java/com/sibyl/api/"


def java_spring(ax, f):
    ws = ax["wiring_source"]
    jproto = "src/main/" + PROTO
    svc = JAVA + "UserGrpcService.java"
    files = {
        "build.gradle.kts": channel.gradle_spring(
            ["org.springframework.boot:spring-boot-starter", "io.grpc:grpc-netty-shaded:1.68.0",
             "io.grpc:grpc-protobuf:1.68.0", "io.grpc:grpc-stub:1.68.0",
             "com.google.protobuf:protobuf-java:4.28.2"],
            ['    id("com.google.protobuf") version "0.9.4"']),
        jproto: proto(f),
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
        JAVA + "GrpcConfig.java": fill(
            """
            package com.sibyl.api;

            import io.grpc.Server;
            import io.grpc.ServerBuilder;
            import org.springframework.context.annotation.Bean;
            import org.springframework.context.annotation.Configuration;

            @Configuration
            public class GrpcConfig {
                @Bean(initMethod = "start", destroyMethod = "shutdown")
                public Server grpcServer(UserGrpcService users) {
                    return ServerBuilder.forPort(<<port>>).addService(users).build();
                }
            }
            """,
            port=pick(ws, "50051", "GrpcPorts.USERS"),
        ),
        svc: fill(
            """
            package com.sibyl.api;

            import com.sibyl.users.v1.GetUserRequest;
            import com.sibyl.users.v1.User;
            import com.sibyl.users.v1.UserServiceGrpc;
            import io.grpc.stub.StreamObserver;
            import org.springframework.stereotype.Component;

            @Component
            public class UserGrpcService extends UserServiceGrpc.UserServiceImplBase {
                @Override
                public void getUser(GetUserRequest request, StreamObserver<User> responseObserver) {
                    User user = User.newBuilder()
                            .setId(request.getId())
                            .set<<pascal>>("<<ada>>")
                            .setEmail("<<mail>>")
                            .build();
                    responseObserver.onNext(user);
                    responseObserver.onCompleted();
                }
            }
            """,
            pascal=f.pascal, ada=ADA, mail=ADA_EMAIL,
        ),
    }
    if ws == "const":
        files[JAVA + "GrpcPorts.java"] = fill(
            """
            package com.sibyl.api;

            public final class GrpcPorts {
                public static final int USERS = 50051;

                private GrpcPorts() {}
            }
            """
        )
    return files, {"route_file": jproto, "handler": "getUser", "handler_file": svc,
                   "model_file": jproto, "pair_files": [jproto, svc], "eco": "gradle",
                   "settings": channel.GRADLE_SETTINGS}


def rust_axum(ax, f):
    ws = ax["wiring_source"]
    files = {
        "Cargo.toml": channel.cargo([("prost", '"0.13"'), ("tonic", '"0.12"'),
                                     ("tokio", '{ version = "1", features = ["full"] }')],
                                    build=[("tonic-build", '"0.12"')]),
        "build.rs": fill(
            """
            fn main() -> Result<(), Box<dyn std::error::Error>> {
                tonic_build::compile_protos("<<proto>>")?;
                Ok(())
            }
            """,
            proto=PROTO,
        ),
        PROTO: proto(f),
        "src/main.rs": fill(
            """
            use tonic::transport::Server;

            mod users;

            use users::pb::user_service_server::UserServiceServer;
            <<konst>>
            #[tokio::main]
            async fn main() -> Result<(), Box<dyn std::error::Error>> {
                let addr = <<addr>>.parse()?;
                Server::builder()
                    .add_service(UserServiceServer::new(users::Users::default()))
                    .serve(addr)
                    .await?;
                Ok(())
            }
            """,
            konst=pick(ws, "", '\nconst GRPC_ADDR: &str = "0.0.0.0:50051";\n'),
            addr=pick(ws, '"0.0.0.0:50051"', "GRPC_ADDR"),
        ),
        "src/users.rs": fill(
            """
            use tonic::{Request, Response, Status};

            pub mod pb {
                tonic::include_proto!("users.v1");
            }

            use pb::user_service_server::UserService;
            use pb::{GetUserRequest, User};

            #[derive(Default)]
            pub struct Users;

            #[tonic::async_trait]
            impl UserService for Users {
                async fn get_user(&self, request: Request<GetUserRequest>) -> Result<Response<User>, Status> {
                    let id = request.into_inner().id;
                    Ok(Response::new(User {
                        id,
                        <<snake>>: "<<ada>>".to_string(),
                        email: "<<mail>>".to_string(),
                    }))
                }
            }
            """,
            snake=f.snake, ada=ADA, mail=ADA_EMAIL,
        ),
    }
    return files, {"route_file": PROTO, "handler": "get_user", "handler_file": "src/users.rs",
                   "model_file": PROTO, "pair_files": [PROTO, "src/users.rs"], "eco": "cargo"}


def csharp_aspnet(ax, f):
    ws = ax["wiring_source"]
    cproto, svc = "Protos/users.proto", "Services/UserGrpcService.cs"
    files = {
        "Api.csproj": channel.csproj(
            [("Grpc.AspNetCore", "2.66.0")],
            '    <Protobuf Include="Protos\\users.proto" GrpcServices="Server" />\n'),
        cproto: proto(f),
        "Program.cs": fill(
            """
            using Microsoft.AspNetCore.Server.Kestrel.Core;
            using Sibyl.Api.Services;

            var builder = WebApplication.CreateBuilder(args);
            builder.WebHost.ConfigureKestrel(o => o.ListenAnyIP(<<port>>, l => l.Protocols = HttpProtocols.Http2));
            builder.Services.AddGrpc();
            var app = builder.Build();
            app.MapGrpcService<UserGrpcService>();
            app.Run();
            """,
            port=pick(ws, "50051", "Sibyl.Api.GrpcPorts.Users"),
        ),
        svc: fill(
            """
            using Grpc.Core;
            using Sibyl.Users.V1;

            namespace Sibyl.Api.Services;

            public class UserGrpcService : UserService.UserServiceBase
            {
                public override Task<User> GetUser(GetUserRequest request, ServerCallContext context) =>
                    Task.FromResult(new User { Id = request.Id, <<pascal>> = "<<ada>>", Email = "<<mail>>" });
            }
            """,
            pascal=f.pascal, ada=ADA, mail=ADA_EMAIL,
        ),
    }
    if ws == "const":
        files["GrpcPorts.cs"] = fill(
            """
            namespace Sibyl.Api;

            public static class GrpcPorts
            {
                public const int Users = 50051;
            }
            """
        )
    return files, {"route_file": cproto, "handler": "GetUser", "handler_file": svc,
                   "model_file": cproto, "pair_files": [cproto, svc], "eco": "dotnet"}


SERVERS = {"go-gin": go_gin, "ts-nest": ts_nest, "java-spring": java_spring,
           "rust-axum": rust_axum, "csharp-aspnet": csharp_aspnet}


# ------------------------------------------------------------------- generated client code

def _pb_bytes(f):
    """users_pb2's serialized FileDescriptorProto, encoded here exactly as protoc would."""
    def varint(n):
        out = b""
        while True:
            b, n = n & 0x7F, n >> 7
            out += bytes([b | (0x80 if n else 0)])
            if not n:
                return out

    def ld(tag, payload):
        payload = payload.encode() if isinstance(payload, str) else payload
        return varint(tag << 3 | 2) + varint(len(payload)) + payload

    def vi(tag, n):
        return varint(tag << 3) + varint(n)

    def fld(name, num, json):
        return ld(1, name) + vi(3, num) + vi(4, 1) + vi(5, 9) + ld(10, json)

    req = ld(1, "GetUserRequest") + ld(2, fld("id", 1, "id"))
    user = (ld(1, "User") + ld(2, fld("id", 1, "id")) + ld(2, fld(f.snake, 2, f.camel))
            + ld(2, fld("email", 3, "email")))
    svc = ld(1, "UserService") + ld(2, ld(1, "GetUser") + ld(2, ".users.v1.GetUserRequest")
                                     + ld(3, ".users.v1.User"))
    opts = (ld(1, "com.sibyl.users.v1") + vi(10, 1)
            + ld(11, "example.com/sibyl/api/gen/users/v1;usersv1") + ld(37, "Sibyl.Users.V1"))
    return (ld(1, "users/v1/users.proto") + ld(2, "users.v1") + ld(4, req) + ld(4, user)
            + ld(6, svc) + ld(8, opts) + ld(12, "proto3"))


def gen_ts(f):
    return {"generated/users.ts": fill(
        """
        // Code generated by protoc-gen-ts_proto. DO NOT EDIT.
        // source: users/v1/users.proto

        /* eslint-disable */
        import {
          type ChannelCredentials,
          Client,
          type ClientUnaryCall,
          makeGenericClientConstructor,
          type ServiceError,
        } from '@grpc/grpc-js';

        export interface GetUserRequest {
          id: string;
        }

        export interface User {
          id: string;
          <<f>>: string;
          email: string;
        }

        export const UserServiceService = {
          getUser: {
            path: '<<method>>',
            requestStream: false,
            responseStream: false,
          },
        } as const;

        export interface UserServiceClient extends Client {
          getUser(
            request: GetUserRequest,
            callback: (error: ServiceError | null, response: User) => void,
          ): ClientUnaryCall;
        }

        export const UserServiceClient = makeGenericClientConstructor(
          UserServiceService as never,
          '<<service>>',
        ) as unknown as { new (address: string, credentials: ChannelCredentials): UserServiceClient };
        """,
        f=f.camel, method=METHOD, service=SERVICE,
    )}


DART_HEAD = ("//\n//  Generated code. Do not modify.\n//  source: users/v1/users.proto\n//\n\n"
             "// ignore_for_file: annotate_overrides, camel_case_types\n\n")


def gen_dart(f):
    return {
        "generated/users.pb.dart": DART_HEAD + fill(
            """
            import 'package:protobuf/protobuf.dart' as $pb;

            class GetUserRequest extends $pb.GeneratedMessage {
              factory GetUserRequest({String? id}) {
                final result = create();
                if (id != null) result.id = id;
                return result;
              }
              GetUserRequest._() : super();

              static GetUserRequest create() => GetUserRequest._();
              static final $pb.BuilderInfo _i = $pb.BuilderInfo('GetUserRequest',
                  package: const $pb.PackageName('users.v1'), createEmptyInstance: create)
                ..aOS(1, 'id');

              $pb.BuilderInfo get info_ => _i;
              GetUserRequest clone() => GetUserRequest()..mergeFromMessage(this);
              GetUserRequest createEmptyInstance() => create();

              String get id => $_getSZ(0);
              set id(String v) => $_setString(0, v);
            }

            class User extends $pb.GeneratedMessage {
              User._() : super();

              static User create() => User._();
              static User fromBuffer(List<int> i) => create()..mergeFromBuffer(i);
              static final $pb.BuilderInfo _i = $pb.BuilderInfo('User',
                  package: const $pb.PackageName('users.v1'), createEmptyInstance: create)
                ..aOS(1, 'id')
                ..aOS(2, '<<f>>')
                ..aOS(3, 'email');

              $pb.BuilderInfo get info_ => _i;
              User clone() => create()..mergeFromMessage(this);
              User createEmptyInstance() => create();

              String get id => $_getSZ(0);
              String get <<f>> => $_getSZ(1);
              String get email => $_getSZ(2);
            }
            """,
            f=f.camel,
        ),
        "generated/users.pbgrpc.dart": DART_HEAD + fill(
            """
            import 'dart:core' as $core;

            import 'package:grpc/service_api.dart' as $grpc;

            import 'users.pb.dart' as $0;

            export 'users.pb.dart';

            class UserServiceClient extends $grpc.Client {
              static final _$getUser = $grpc.ClientMethod<$0.GetUserRequest, $0.User>(
                  '<<method>>',
                  ($0.GetUserRequest value) => value.writeToBuffer(),
                  ($core.List<$core.int> value) => $0.User.fromBuffer(value));

              UserServiceClient($grpc.ClientChannel channel, {$grpc.CallOptions? options})
                  : super(channel, options: options);

              $grpc.ResponseFuture<$0.User> getUser($0.GetUserRequest request,
                  {$grpc.CallOptions? options}) {
                return $createUnaryCall(_$getUser, request, options: options);
              }
            }
            """,
            method=METHOD,
        ),
    }


KT_PB = "com/sibyl/users/v1/"


def gen_kotlin(f):
    return {
        KT_PB + "Users.kt": fill(
            """
            // Generated by the protocol buffer compiler. DO NOT EDIT!
            // source: users/v1/users.proto
            package com.sibyl.users.v1

            public data class GetUserRequest(
                public val id: String = "",
            )

            public data class User(
                public val id: String = "",
                public val <<f>>: String = "",
                public val email: String = "",
            )
            """,
            f=f.camel,
        ),
        KT_PB + "UserServiceGrpcKt.kt": fill(
            """
            // Generated by the gRPC Kotlin plugin. DO NOT EDIT!
            // source: users/v1/users.proto
            package com.sibyl.users.v1

            import io.grpc.CallOptions
            import io.grpc.Channel
            import io.grpc.kotlin.AbstractCoroutineStub
            import io.grpc.kotlin.ClientCalls.unaryRpc

            public object UserServiceGrpcKt {
                public const val SERVICE_NAME: String = "<<service>>"

                public class UserServiceCoroutineStub @JvmOverloads constructor(
                    channel: Channel,
                    callOptions: CallOptions = CallOptions.DEFAULT,
                ) : AbstractCoroutineStub<UserServiceCoroutineStub>(channel, callOptions) {
                    override fun build(channel: Channel, callOptions: CallOptions): UserServiceCoroutineStub =
                        UserServiceCoroutineStub(channel, callOptions)

                    public suspend fun getUser(request: GetUserRequest): User =
                        unaryRpc(channel, UserServiceGrpc.getGetUserMethod(), request, callOptions)
                }
            }
            """,
            service=SERVICE,
        ),
    }


SWIFT_PB_HEAD = ("// DO NOT EDIT.\n// swift-format-ignore-file\n//\n"
                 "// Generated by the Swift generator plugin for the protocol buffer compiler.\n"
                 "// Source: users/v1/users.proto\n\n")


def gen_swift(f):
    return {
        "Generated/users.pb.swift": SWIFT_PB_HEAD + fill(
            """
            import SwiftProtobuf

            public struct Users_V1_GetUserRequest: Sendable {
                public var id: String = String()

                public init() {}
            }

            public struct Users_V1_User: Sendable {
                public var id: String = String()

                public var <<f>>: String = String()

                public var email: String = String()

                public init() {}
            }
            """,
            f=f.camel,
        ),
        "Generated/users.grpc.swift": SWIFT_PB_HEAD + fill(
            """
            import GRPC
            import NIO
            import SwiftProtobuf

            public protocol Users_V1_UserServiceClientProtocol: GRPCClient {
                func getUser(
                    _ request: Users_V1_GetUserRequest,
                    callOptions: CallOptions?
                ) -> UnaryCall<Users_V1_GetUserRequest, Users_V1_User>
            }

            public final class Users_V1_UserServiceNIOClient: Users_V1_UserServiceClientProtocol {
                public let channel: GRPCChannel
                public var defaultCallOptions: CallOptions

                public init(channel: GRPCChannel, defaultCallOptions: CallOptions = CallOptions()) {
                    self.channel = channel
                    self.defaultCallOptions = defaultCallOptions
                }

                public func getUser(
                    _ request: Users_V1_GetUserRequest,
                    callOptions: CallOptions? = nil
                ) -> UnaryCall<Users_V1_GetUserRequest, Users_V1_User> {
                    return self.makeUnaryCall(
                        path: "<<method>>",
                        request: request,
                        callOptions: callOptions ?? self.defaultCallOptions
                    )
                }
            }
            """,
            method=METHOD,
        ),
    }


def gen_python(f):
    return {
        "generated/__init__.py": "",
        "generated/users_pb2.py": fill(
            """
            # -*- coding: utf-8 -*-
            # Generated by the protocol buffer compiler.  DO NOT EDIT!
            # source: users/v1/users.proto
            \"\"\"Generated protocol buffer code.\"\"\"
            from google.protobuf import descriptor_pool as _descriptor_pool
            from google.protobuf import symbol_database as _symbol_database
            from google.protobuf.internal import builder as _builder

            _sym_db = _symbol_database.Default()


            DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(<<bytes>>)

            _globals = globals()
            _builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, _globals)
            _builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, 'users_pb2', _globals)
            """,
            bytes=repr(_pb_bytes(f)),
        ),
        "generated/users_pb2_grpc.py": fill(
            """
            # Generated by the gRPC Python protocol compiler plugin. DO NOT EDIT!
            \"\"\"Client and server classes corresponding to protobuf-defined services.\"\"\"
            import grpc

            from . import users_pb2 as users__pb2


            class UserServiceStub(object):
                \"\"\"Missing associated documentation comment in .proto file.\"\"\"

                def __init__(self, channel):
                    \"\"\"Constructor.

                    Args:
                        channel: A grpc.Channel.
                    \"\"\"
                    self.GetUser = channel.unary_unary(
                            '<<method>>',
                            request_serializer=users__pb2.GetUserRequest.SerializeToString,
                            response_deserializer=users__pb2.User.FromString,
                            _registered_method=True)
            """,
            method=METHOD,
        ),
    }


# ---------------------------------------------------------------------------------- clients

def _addr(lang, ws, o, imports):
    """The dial target: (host expression, port) for Dart and Swift, which take them apart,
    else the `host:port` expression; `const` holds the host (or target) in a constant."""
    split = lang in ("dart", "swift")
    value = "localhost" if split else ADDR
    if ws == "literal":
        expr = channel.strlit(lang, [("lit", value)])
    else:
        pick(ws, None, "const")
        name = channel.const_name(lang, "USERS_GRPC_HOST" if split else "USERS_GRPC_ADDR")
        rel, text, imp = konst(lang, name, value)
        o.api[rel] = text
        if imp and imp not in imports:
            imports.append(imp)
        expr = name
    return (expr, 50051) if split else expr


def ts(ax, f, pack):
    ws, call, contract = ax["wiring_source"], ax["wiring_call"], ax["contract"]
    o = Out(deps=["@grpc/grpc-js"])
    o.api.update(gen_ts(f))
    imports = ["import { credentials } from '@grpc/grpc-js';"]
    if call == "generated-sdk":
        imports.append("import { UserServiceClient } from './generated/users';")
        addr = _addr("ts", ws, o, imports)
        o.api["client.ts"] = block(
            *imports, "",
            f"export const users = new UserServiceClient({addr}, credentials.createInsecure());")
        o.call, o.exports = "client.ts", ["client", "generated/users"]
        o.app["profile.ts"] = fill(
            """
            <<imp>>

            export function showUser(id: string, render: (label: string) => void): void {
              users.getUser({ id }, (err, user) => {
                if (err) {
                  return;
                }
                render(`${user.<<f>>} <${user.email}>`);
              });
            }
            """,
            imp=app_import("ts", pack, "client", ["users"]), f=f.camel,
        )
    else:
        declared = contract == "declared-type"
        imports.append("import { UserServiceClient, type User as UserMessage } from "
                       "'./generated/users';" if declared else
                       "import { UserServiceClient, type User } from './generated/users';")
        if declared:
            o.api.update(payload.model("ts", contract, f, "").files)
            imports.append("import type { User } from './models';")
        addr = _addr("ts", ws, o, imports)
        ret = (f"{{ id: res.id, {f.camel}: res.{f.camel}, email: res.email }}" if declared
               else "res")
        o.api["users.ts"] = block(*imports, "", fill(
            """
            const stub = new UserServiceClient(<<addr>>, credentials.createInsecure());

            export function fetchUser(id: string): Promise<User> {
              return new Promise((resolve, reject) => {
                stub.getUser({ id }, (err, res<<ann>>) => (err ? reject(err) : resolve(<<ret>>)));
              });
            }
            """,
            addr=addr, ret=ret, ann=": UserMessage" if declared else "",
        ))
        o.call, o.exports = "users.ts", ["users"]
        o.app["profile.ts"] = fill(
            """
            <<imp>>

            export async function userLabel(id: string): Promise<string> {
              const user = await fetchUser(id);
              return `${user.<<f>>} <${user.email}>`;
            }
            """,
            imp=app_import("ts", pack, "users", ["fetchUser"]), f=f.camel,
        )
    o.reader = "profile.ts"
    return o


def dart(ax, f, pack):
    ws, call, contract = ax["wiring_source"], ax["wiring_call"], ax["contract"]
    o = Out(deps=["grpc", "protobuf"])
    o.api.update(gen_dart(f))
    imports = ["import 'package:grpc/grpc.dart';"]
    chan = ("ClientChannel(<<host>>, port: <<port>>,\n"
            "    options: const ChannelOptions(credentials: ChannelCredentials.insecure()))")
    if call == "generated-sdk":
        imports.append("import 'generated/users.pbgrpc.dart';")
        host, port = _addr("dart", ws, o, imports)
        o.api["client.dart"] = block(*sorted(set(imports)), "",
                                     "final users = UserServiceClient(" + fill(chan, host=host, port=port) + ");")
        o.call = "client.dart"
        o.exports = ["client.dart", "generated/users.pbgrpc.dart"]
        mod, extra = "client.dart", (None if pack == "workspace"
                                     else "import 'api/generated/users.pbgrpc.dart';")
        fetch = "final user = await users.getUser(GetUserRequest(id: id));"
    else:
        declared = contract == "declared-type"
        imports.append("import 'generated/users.pbgrpc.dart'" + (" as pb;" if declared else ";"))
        if declared:
            o.api.update(payload.model("dart", contract, f, "").files)
            imports.append("import 'user.dart';")
        host, port = _addr("dart", ws, o, imports)
        q = "pb." if declared else ""
        ret = (f"User(id: res.id, {f.camel}: res.{f.camel}, email: res.email)" if declared
               else "res")
        o.api["users.dart"] = block(*sorted(set(imports)), "", fill(
            """
            final _stub = <<q>>UserServiceClient(<<chan>>);

            Future<User> fetchUser(String id) async {
              final res = await _stub.getUser(<<q>>GetUserRequest(id: id));
              return <<ret>>;
            }
            """,
            q=q, chan=fill(chan, host=host, port=port), ret=ret,
        ))
        o.call, o.exports, mod, extra = "users.dart", ["users.dart"], "users.dart", None
        fetch = "final user = await fetchUser(id);"
    o.app["profile.dart"] = block(
        app_import("dart", pack, mod, []), extra, "",
        "Future<String> userLabel(String id) async {", f"  {fetch}",
        f"  return '${{user.{f.camel}}} <${{user.email}}>';", "}")
    o.reader = "profile.dart"
    return o


def kotlin(ax, f, pack):
    ws, call, contract = ax["wiring_source"], ax["wiring_call"], ax["contract"]
    o = Out(deps=["grpc-kotlin-stub", "grpc-okhttp", "protobuf-kotlin"])
    o.api.update(gen_kotlin(f))
    imports = ["import com.sibyl.users.v1.UserServiceGrpcKt", "import io.grpc.ManagedChannelBuilder"]
    if call == "generated-sdk":
        addr = _addr("kotlin", ws, o, imports)
        o.api[KT_API + "Client.kt"] = block(
            "package com.sibyl.api", "", *sorted(imports), "",
            "val users = UserServiceGrpcKt.UserServiceCoroutineStub(",
            f"    ManagedChannelBuilder.forTarget({addr}).usePlaintext().build(),", ")")
        o.call = KT_API + "Client.kt"
        app = block("package com.sibyl.web", "", "import com.sibyl.api.users",
                    "import com.sibyl.users.v1.GetUserRequest", "",
                    "suspend fun userLabel(id: String): String {",
                    "    val user = users.getUser(GetUserRequest(id = id))",
                    f'    return "${{user.{f.camel}}} <${{user.email}}>"', "}")
    else:
        declared = contract == "declared-type"
        imports.append("import com.sibyl.users.v1.GetUserRequest")
        if declared:
            o.api.update(payload.model("kotlin", contract, f, "").files)
        else:
            imports.append("import com.sibyl.users.v1.User")
        addr = _addr("kotlin", ws, o, imports)
        ret = (f"User(id = res.id, {f.camel} = res.{f.camel}, email = res.email)" if declared
               else "res")
        o.api[KT_API + "Users.kt"] = block("package com.sibyl.api", "", *sorted(set(imports)), "", fill(
            """
            private val stub = UserServiceGrpcKt.UserServiceCoroutineStub(
                ManagedChannelBuilder.forTarget(<<addr>>).usePlaintext().build(),
            )

            suspend fun fetchUser(id: String): User {
                val res = stub.getUser(GetUserRequest(id = id))
                return <<ret>>
            }
            """,
            addr=addr, ret=ret,
        ))
        o.call = KT_API + "Users.kt"
        app = block("package com.sibyl.web", "", "import com.sibyl.api.fetchUser", "",
                    "suspend fun userLabel(id: String): String {", "    val user = fetchUser(id)",
                    f'    return "${{user.{f.camel}}} <${{user.email}}>"', "}")
    o.app["com/sibyl/web/Profile.kt"] = app
    o.reader = "com/sibyl/web/Profile.kt"
    return o


def swift(ax, f, pack):
    ws, call, contract = ax["wiring_source"], ax["wiring_call"], ax["contract"]
    o = Out(deps=["grpc-swift", "swift-protobuf"])
    o.api.update(gen_swift(f))
    host, port = _addr("swift", ws, o, [])
    stub = fill(
        """
        let group = MultiThreadedEventLoopGroup(numberOfThreads: 1)

        <<vis>>let users = Users_V1_UserServiceNIOClient(
            channel: try! GRPCChannelPool.with(
                target: .host(<<host>>, port: <<port>>),
                transportSecurity: .plaintext,
                eventLoopGroup: group
            )
        )
        """,
        vis="public " if call == "generated-sdk" else "private ", host=host, port=port,
    )
    head = app_import("swift", pack, "", [])
    if call == "generated-sdk":
        o.api["Client.swift"] = block("import GRPC", "import NIO", "", stub)
        o.call = "Client.swift"
        o.app["Profile.swift"] = block(
            head, "" if head else None,
            "public func userLabel(id: String) async throws -> String {",
            "    var request = Users_V1_GetUserRequest()", "    request.id = id",
            "    let user = try await users.getUser(request).response.get()",
            f'    return "\\(user.{f.camel}) <\\(user.email)>"', "}")
    else:
        declared = contract == "declared-type"
        if declared:
            o.api.update(payload.model("swift", contract, f, "").files)
        ret = (f"User(id: res.id, {f.camel}: res.{f.camel}, email: res.email)" if declared
               else "res")
        ty = "User" if declared else "Users_V1_User"
        o.api["Users.swift"] = block("import GRPC", "import NIO", "", stub, "", fill(
            """
            public func fetchUser(id: String) async throws -> <<ty>> {
                var request = Users_V1_GetUserRequest()
                request.id = id
                let res = try await users.getUser(request).response.get()
                return <<ret>>
            }
            """,
            ty=ty, ret=ret,
        ))
        o.call = "Users.swift"
        o.app["Profile.swift"] = block(
            head, "" if head else None,
            "public func userLabel(id: String) async throws -> String {",
            "    let user = try await fetchUser(id: id)",
            f'    return "\\(user.{f.camel}) <\\(user.email)>"', "}")
    o.reader = "Profile.swift"
    return o


def python(ax, f, pack):
    ws, call, contract = ax["wiring_source"], ax["wiring_call"], ax["contract"]
    o = Out(deps=["grpcio", "python-protobuf"])
    o.api.update(gen_python(f))
    imports = ["import grpc", "", "from .generated import users_pb2, users_pb2_grpc"]
    if call == "generated-sdk":
        addr = _addr("python", ws, o, imports)
        o.api["client.py"] = block(
            *imports, "", f"users = users_pb2_grpc.UserServiceStub(grpc.insecure_channel({addr}))")
        o.call, mod, use = "client.py", "client", ["users"]
        fetch = "user = users.GetUser(users_pb2.GetUserRequest(id=user_id))"
        extra = ("from sibyl_api_client.generated import users_pb2" if pack == "workspace"
                 else "from .api.generated import users_pb2")
    else:
        declared = contract == "declared-type"
        if declared:
            o.api.update(payload.model("python", contract, f, "").files)
            imports.append("from .models import User")
        addr = _addr("python", ws, o, imports)
        ret = (f"User(id=res.id, {f.snake}=res.{f.snake}, email=res.email)" if declared
               else "res")
        ty = "User" if declared else "users_pb2.User"
        o.api["users.py"] = block(*imports, "", fill(
            """
            _stub = users_pb2_grpc.UserServiceStub(grpc.insecure_channel(<<addr>>))


            def fetch_user(user_id: str) -> <<ty>>:
                res = _stub.GetUser(users_pb2.GetUserRequest(id=user_id))
                return <<ret>>
            """,
            addr=addr, ty=ty, ret=ret,
        ))
        o.call, mod, use, extra = "users.py", "users", ["fetch_user"], None
        fetch = "user = fetch_user(user_id)"
    o.app["profile.py"] = block(
        app_import("python", pack, mod, use), extra, "", "",
        "def user_label(user_id: str) -> str:", f"    {fetch}",
        f'    return f"{{user.{f.snake}}} <{{user.email}}>"')
    o.reader = "profile.py"
    return o


CLIENTS = {"ts": ts, "dart": dart, "kotlin": kotlin, "swift": swift, "python": python}


def _client(ax, f, pack):
    return CLIENTS[ax["client"]](ax, f, pack)


def _server(ax, f):
    return SERVERS[ax["server"]](ax, f)


def render(case, state):
    return channel.assemble(case["axes"], state, _client, _server)[0]


def expected(case):
    return channel.expected(case, "grpc", CATEGORY, EXCHANGE, _client, _server,
                            "the User message")
