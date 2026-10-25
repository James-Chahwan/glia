"""REST servers: one function per `server` axis value. Each writes the server side of the one
exchange (GET /api/v1/users/{id} -> User) with its route path built the `wiring_source` way,
and returns (files, meta): files keyed by path relative to the server's root, meta naming the
route file, the handler and its file, the model file and the server's ecosystem."""

import json

from .common import PREFIX, fill

ADA = "Ada Lovelace"
ADA_EMAIL = "ada@example.com"


def _pkg_json(name, deps, dev=None):
    body = {"name": name, "version": "0.1.0", "private": True, "dependencies": deps}
    if dev:
        body["devDependencies"] = dev
    return json.dumps(body, indent=2) + "\n"


TSCONFIG = json.dumps(
    {
        "compilerOptions": {
            "target": "ES2022",
            "module": "commonjs",
            "strict": True,
            "experimentalDecorators": True,
            "emitDecoratorMetadata": True,
            "outDir": "dist",
        },
        "include": ["src"],
    },
    indent=2,
) + "\n"


def go_gin(ws, f):
    reg = {
        "literal": '\tr.GET("/api/v1/users/:id", getUser)',
        "const": '\tapi := r.Group(apiPrefix)\n\tapi.GET("/users/:id", getUser)',
        "enum-prefix": '\tv1 := r.Group("/api/v1")\n'
        '\tusers := v1.Group("/" + string(RouteUsers))\n'
        '\tusers.GET("/:id", getUser)',
    }[ws]
    files = {
        "go.mod": "module example.com/sibyl/api\n\ngo 1.22\n\n"
        "require github.com/gin-gonic/gin v1.10.0\n",
        "main.go": fill(
            """
            package main

            import "github.com/gin-gonic/gin"

            func main() {
            \tr := gin.Default()
            <<reg>>
            \tif err := r.Run(":8080"); err != nil {
            \t\tpanic(err)
            \t}
            }
            """,
            reg=reg,
        ),
        "users.go": fill(
            """
            package main

            import (
            \t"net/http"

            \t"github.com/gin-gonic/gin"
            )

            // User is the user resource the API returns.
            type User struct {
            \tID    string `json:"id"`
            \t<<pascal>> string `json:"<<camel>>"`
            \tEmail string `json:"email"`
            }

            func getUser(c *gin.Context) {
            \tc.JSON(http.StatusOK, User{ID: c.Param("id"), <<pascal>>: "<<ada>>", Email: "<<mail>>"})
            }
            """,
            pascal=f.pascal, camel=f.camel, ada=ADA, mail=ADA_EMAIL,
        ),
    }
    if ws == "const":
        files["routes.go"] = 'package main\n\nconst apiPrefix = "/api/v1"\n'
    elif ws == "enum-prefix":
        files["routes.go"] = (
            "package main\n\n// RouteKey names a resource's route segment.\n"
            "type RouteKey string\n\n"
            'const RouteUsers RouteKey = "users"\n'
        )
    meta = {"route_file": "main.go", "handler": "getUser", "handler_file": "users.go",
            "model_file": "users.go", "eco": "go"}
    return files, meta


def _ts_routes(ws, prefix):
    if ws == "const":
        return f"export const API_PREFIX = '{prefix}';\n"
    if ws == "enum-prefix":
        return "export enum RouteKey {\n  Users = 'users',\n}\n"
    return None


def ts_nest(ws, f):
    ctl = {"literal": "'api/v1/users'", "const": "`${API_PREFIX}/users`",
           "enum-prefix": "RouteKey.Users"}[ws]
    imp = {"literal": None, "const": "import { API_PREFIX } from '../routes';",
           "enum-prefix": "import { RouteKey } from '../routes';"}[ws]
    files = {
        "package.json": _pkg_json(
            "sibyl-api",
            {"@nestjs/common": "^10.4.0", "@nestjs/core": "^10.4.0",
             "@nestjs/platform-express": "^10.4.0", "reflect-metadata": "^0.2.2",
             "rxjs": "^7.8.1"},
            {"typescript": "^5.6.0"},
        ),
        "tsconfig.json": TSCONFIG,
        "src/main.ts": fill(
            """
            import { NestFactory } from '@nestjs/core';
            import { AppModule } from './app.module';

            async function bootstrap() {
              const app = await NestFactory.create(AppModule);
            <<prefix>>  await app.listen(3000);
            }

            void bootstrap();
            """,
            prefix="  app.setGlobalPrefix('api/v1');\n" if ws == "enum-prefix" else "",
        ),
        "src/app.module.ts": fill(
            """
            import { Module } from '@nestjs/common';
            import { UsersController } from './users/users.controller';

            @Module({ controllers: [UsersController] })
            export class AppModule {}
            """
        ),
        "src/users/users.controller.ts": fill(
            """
            import { Controller, Get, Param } from '@nestjs/common';
            <<imp>>import { UserDto } from './user.dto';

            @Controller(<<ctl>>)
            export class UsersController {
              @Get(':id')
              getUser(@Param('id') id: string): UserDto {
                return { id, <<camel>>: '<<ada>>', email: '<<mail>>' };
              }
            }
            """,
            imp=imp + "\n" if imp else "", ctl=ctl, camel=f.camel, ada=ADA, mail=ADA_EMAIL,
        ),
        "src/users/user.dto.ts": fill(
            """
            export class UserDto {
              id!: string;
              <<camel>>!: string;
              email!: string;
            }
            """,
            camel=f.camel,
        ),
    }
    routes = _ts_routes(ws, "api/v1")
    if routes:
        files["src/routes.ts"] = routes
    ctl_file = "src/users/users.controller.ts"
    meta = {"route_file": ctl_file, "handler": "getUser", "handler_file": ctl_file,
            "model_file": "src/users/user.dto.ts", "eco": "node"}
    return files, meta


def ts_express(ws, f):
    sub = {"literal": "/api/v1/users/:id", "const": "/users/:id", "enum-prefix": "/:id"}[ws]
    mount = {"literal": "", "const": "API_PREFIX, ",
             "enum-prefix": "`/api/v1/${RouteKey.Users}`, "}[ws]
    imp = {"literal": "", "const": "import { API_PREFIX } from './routes';\n",
           "enum-prefix": "import { RouteKey } from './routes';\n"}[ws]
    files = {
        "package.json": _pkg_json("sibyl-api", {"express": "^4.21.0"},
                                  {"@types/express": "^4.17.21", "typescript": "^5.6.0"}),
        "tsconfig.json": TSCONFIG,
        "src/app.ts": fill(
            """
            import express from 'express';
            <<imp>>import { usersRouter } from './users';

            const app = express();
            app.use(<<mount>>usersRouter);
            app.listen(3000);
            """,
            imp=imp, mount=mount,
        ),
        "src/users.ts": fill(
            """
            import { Router, type Request, type Response } from 'express';

            export interface User {
              id: string;
              <<camel>>: string;
              email: string;
            }

            export const usersRouter = Router();

            export function getUser(req: Request, res: Response): void {
              const user: User = { id: req.params.id, <<camel>>: '<<ada>>', email: '<<mail>>' };
              res.json(user);
            }

            usersRouter.get('<<sub>>', getUser);
            """,
            camel=f.camel, ada=ADA, mail=ADA_EMAIL, sub=sub,
        ),
    }
    routes = _ts_routes(ws, PREFIX)
    if routes:
        files["src/routes.ts"] = routes
    meta = {"route_file": "src/users.ts", "handler": "getUser", "handler_file": "src/users.ts",
            "model_file": "src/users.ts", "eco": "node"}
    return files, meta


JAVA_DIR = "src/main/java/com/sibyl/api/"


def java_spring(ws, f):
    rm = {"literal": '"/api/v1/users"', "const": 'ApiPaths.API_PREFIX + "/users"'}[ws]
    files = {
        "build.gradle.kts": fill(
            """
            plugins {
                java
                id("org.springframework.boot") version "3.3.4"
                id("io.spring.dependency-management") version "1.1.6"
            }

            repositories {
                mavenCentral()
            }

            dependencies {
                implementation("org.springframework.boot:spring-boot-starter-web")
            }
            """
        ),
        JAVA_DIR + "Application.java": fill(
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
        JAVA_DIR + "UsersController.java": fill(
            """
            package com.sibyl.api;

            import org.springframework.web.bind.annotation.GetMapping;
            import org.springframework.web.bind.annotation.PathVariable;
            import org.springframework.web.bind.annotation.RequestMapping;
            import org.springframework.web.bind.annotation.RestController;

            @RestController
            @RequestMapping(<<rm>>)
            public class UsersController {

                @GetMapping("/{id}")
                public UserDto getUser(@PathVariable String id) {
                    return new UserDto(id, "<<ada>>", "<<mail>>");
                }
            }
            """,
            rm=rm, ada=ADA, mail=ADA_EMAIL,
        ),
        JAVA_DIR + "UserDto.java": fill(
            """
            package com.sibyl.api;

            public record UserDto(String id, String <<camel>>, String email) {}
            """,
            camel=f.camel,
        ),
    }
    if ws == "const":
        files[JAVA_DIR + "ApiPaths.java"] = fill(
            """
            package com.sibyl.api;

            public final class ApiPaths {
                public static final String API_PREFIX = "/api/v1";

                private ApiPaths() {}
            }
            """
        )
    ctl = JAVA_DIR + "UsersController.java"
    meta = {"route_file": ctl, "handler": "getUser", "handler_file": ctl,
            "model_file": JAVA_DIR + "UserDto.java", "eco": "gradle",
            "settings": 'rootProject.name = "sibyl-api"\n'}
    return files, meta


def python_fastapi(ws, f):
    prefix = {"literal": '"/api/v1/users"', "const": 'f"{API_PREFIX}/users"',
              "enum-prefix": 'f"/api/v1/{RouteKey.USERS}"'}[ws]
    imp = {"literal": "", "const": "from .routes import API_PREFIX\n",
           "enum-prefix": "from .routes import RouteKey\n"}[ws]
    files = {
        "pyproject.toml": fill(
            """
            [project]
            name = "sibyl-api"
            version = "0.1.0"
            requires-python = ">=3.11"
            dependencies = ["fastapi>=0.115", "pydantic>=2.8"]
            """
        ),
        "app/__init__.py": "",
        "app/main.py": fill(
            """
            from fastapi import FastAPI

            from . import users

            app = FastAPI()
            app.include_router(users.router)
            """
        ),
        "app/models.py": fill(
            """
            from pydantic import BaseModel


            class User(BaseModel):
                id: str
                <<camel>>: str
                email: str
            """,
            camel=f.camel,
        ),
        "app/users.py": fill(
            """
            from fastapi import APIRouter

            <<imp>>from .models import User

            router = APIRouter(prefix=<<prefix>>)


            @router.get("/{user_id}", response_model=User)
            def get_user(user_id: str) -> User:
                return User(id=user_id, <<camel>>="<<ada>>", email="<<mail>>")
            """,
            imp=imp, prefix=prefix, camel=f.camel, ada=ADA, mail=ADA_EMAIL,
        ),
    }
    if ws == "const":
        files["app/routes.py"] = 'API_PREFIX = "/api/v1"\n'
    elif ws == "enum-prefix":
        files["app/routes.py"] = (
            'from enum import StrEnum\n\n\nclass RouteKey(StrEnum):\n    USERS = "users"\n'
        )
    meta = {"route_file": "app/users.py", "handler": "get_user", "handler_file": "app/users.py",
            "model_file": "app/models.py", "eco": "python"}
    return files, meta


def rust_axum(ws, f):
    router = {
        "literal": 'Router::new().route("/api/v1/users/{id}", get(users::get_user))',
        "const": "Router::new().nest(\n        API_PREFIX,\n"
        '        Router::new().route("/users/{id}", get(users::get_user)),\n    )',
    }[ws]
    files = {
        "Cargo.toml": fill(
            """
            [package]
            name = "sibyl-api"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            axum = "0.8"
            serde = { version = "1", features = ["derive"] }
            tokio = { version = "1", features = ["full"] }
            """
        ),
        "src/main.rs": fill(
            """
            use axum::{routing::get, Router};

            mod users;
            <<konst>>
            #[tokio::main]
            async fn main() {
                let app = <<router>>;
                let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
                axum::serve(listener, app).await.unwrap();
            }
            """,
            konst='\nconst API_PREFIX: &str = "/api/v1";\n' if ws == "const" else "",
            router=router,
        ),
        "src/users.rs": fill(
            """
            use axum::{extract::Path, Json};
            use serde::Serialize;

            #[derive(Serialize)]
            #[serde(rename_all = "camelCase")]
            pub struct User {
                pub id: String,
                pub <<snake>>: String,
                pub email: String,
            }

            pub async fn get_user(Path(id): Path<String>) -> Json<User> {
                Json(User {
                    id,
                    <<snake>>: "<<ada>>".to_string(),
                    email: "<<mail>>".to_string(),
                })
            }
            """,
            snake=f.snake, ada=ADA, mail=ADA_EMAIL,
        ),
    }
    meta = {"route_file": "src/main.rs", "handler": "get_user", "handler_file": "src/users.rs",
            "model_file": "src/users.rs", "eco": "cargo"}
    return files, meta


def csharp_aspnet(ws, f):
    route = {"literal": '"api/v1/users"', "const": 'ApiPaths.ApiPrefix + "/users"'}[ws]
    files = {
        "Api.csproj": fill(
            """
            <Project Sdk="Microsoft.NET.Sdk.Web">
              <PropertyGroup>
                <TargetFramework>net8.0</TargetFramework>
                <RootNamespace>Sibyl.Api</RootNamespace>
                <Nullable>enable</Nullable>
                <ImplicitUsings>enable</ImplicitUsings>
              </PropertyGroup>
            </Project>
            """
        ),
        "Program.cs": fill(
            """
            var builder = WebApplication.CreateBuilder(args);
            builder.Services.AddControllers();
            var app = builder.Build();
            app.MapControllers();
            app.Run();
            """
        ),
        "Controllers/UsersController.cs": fill(
            """
            using Microsoft.AspNetCore.Mvc;
            using Sibyl.Api.Models;

            namespace Sibyl.Api.Controllers;

            [ApiController]
            [Route(<<route>>)]
            public class UsersController : ControllerBase
            {
                [HttpGet("{id}")]
                public ActionResult<UserDto> GetUser(string id) =>
                    new UserDto(id, "<<ada>>", "<<mail>>");
            }
            """,
            route=route, ada=ADA, mail=ADA_EMAIL,
        ),
        "Models/UserDto.cs": fill(
            """
            namespace Sibyl.Api.Models;

            public record UserDto(string Id, string <<pascal>>, string Email);
            """,
            pascal=f.pascal,
        ),
    }
    if ws == "const":
        files["ApiPaths.cs"] = fill(
            """
            namespace Sibyl.Api;

            public static class ApiPaths
            {
                public const string ApiPrefix = "api/v1";
            }
            """
        )
    ctl = "Controllers/UsersController.cs"
    meta = {"route_file": ctl, "handler": "GetUser", "handler_file": ctl,
            "model_file": "Models/UserDto.cs", "eco": "dotnet"}
    return files, meta


SERVERS = {
    "go-gin": go_gin,
    "ts-nest": ts_nest,
    "ts-express": ts_express,
    "java-spring": java_spring,
    "python-fastapi": python_fastapi,
    "rust-axum": rust_axum,
    "csharp-aspnet": csharp_aspnet,
}
