"""The client's model of the one payload (`User {id, <field>, email}`) per `contract` value, shared
by the gRPC, GraphQL, queue and WebSocket clients: the model file(s), the imports the decoding
file needs, the decode expression over a raw value and the reader's field access.

`model(lang, contract, f, note)` -> Model. The raw value a transport hands `decode` is the
platform's natural one: TypeScript an object (`JSON.parse(..)`), Dart a `Map<String, dynamic>`,
Kotlin the JSON text, Swift `Data`, Python a dict. `note` is the generated-file banner the
transport's generator writes (graphql-codegen, quicktype, ..), used only by `generated`.

The shapes are the REST templates' (rest_clients.py): an interface / data class / struct /
dataclass, Zod / json_serializable / kotlinx.serialization / Codable CodingKeys / Pydantic, an
untyped object, and a generated model file under `generated/`."""

from dataclasses import dataclass, field

from .common import fill

KT_API = "com/sibyl/api/"


@dataclass
class Model:
    files: dict = field(default_factory=dict)   # api-relative
    deps: list = field(default_factory=list)
    imports: list = field(default_factory=list)  # for the api file that decodes
    ty: str = ""
    decode: str = "<<v>>"                       # <<v>> = the raw value
    exports: list = field(default_factory=list)  # api modules the app may import


def read(lang, contract, var, f):
    """The reader's access of the renamed field on the decoded value `var`."""
    plain = contract == "plain-literal"
    if lang == "ts":
        return f"{var}.{f.camel}"
    if lang == "dart":
        return f"{var}['{f.camel}']" if plain else f"{var}.{f.camel}"
    if lang == "kotlin":
        return f'{var}.getString("{f.camel}")' if plain else f"{var}.{f.camel}"
    if lang == "swift":
        if plain:
            return f'({var}["{f.camel}"] as? String ?? "")'
        # Codable CodingKeys: the property keeps its name, the key follows the wire.
        return f"{var}.displayName" if contract == "schema-object" else f"{var}.{f.camel}"
    if lang == "python":
        return f'{var}["{f.camel}"]' if plain else f"{var}.{f.snake}"
    raise KeyError(lang)


def model(lang, contract, f, note):
    return {"ts": _ts, "dart": _dart, "kotlin": _kotlin, "swift": _swift,
            "python": _python}[lang](contract, f, note)


def _ts(contract, f, note):
    m = Model(ty="User", decode="(<<v>>) as User")
    iface = fill(
        """
        export interface User {
          id: string;
          <<f>>: string;
          email: string;
        }
        """,
        f=f.camel,
    )
    if contract == "declared-type":
        m.files["models.ts"] = iface
        m.imports.append("import type { User } from './models';")
        m.exports.append("models")
    elif contract == "generated":
        m.files["generated/models.ts"] = f"/* eslint-disable */\n// {note}\n\n" + iface
        m.imports.append("import type { User } from './generated/models';")
        m.exports.append("generated/models")
    elif contract == "schema-object":
        m.deps.append("zod")
        m.files["models.ts"] = fill(
            """
            import { z } from 'zod';

            export const UserSchema = z.object({
              id: z.string(),
              <<f>>: z.string(),
              email: z.string(),
            });

            export type User = z.infer<typeof UserSchema>;
            """,
            f=f.camel,
        )
        m.imports.append("import { UserSchema, type User } from './models';")
        m.decode = "UserSchema.parse(<<v>>)"
        m.exports.append("models")
    else:
        m.ty, m.decode = "any", "<<v>>"
    return m


def _dart_class(f, head=""):
    return head + fill(
        """
        class User {
          const User({required this.id, required this.<<f>>, required this.email});

          factory User.fromJson(Map<String, dynamic> json) => User(
                id: json['id'] as String,
                <<f>>: json['<<f>>'] as String,
                email: json['email'] as String,
              );

          final String id;
          final String <<f>>;
          final String email;
        }
        """,
        f=f.camel,
    )


def _dart(contract, f, note):
    m = Model(ty="User", decode="User.fromJson(<<v>>)")
    if contract == "declared-type":
        m.files["user.dart"] = _dart_class(f)
        m.imports.append("import 'user.dart';")
        m.exports.append("user.dart")
    elif contract == "generated":
        m.files["generated/user.dart"] = _dart_class(
            f, f"// GENERATED CODE - DO NOT MODIFY BY HAND\n// {note}\n\n")
        m.imports.append("import 'generated/user.dart';")
        m.exports.append("generated/user.dart")
    elif contract == "schema-object":
        m.deps.append("json_annotation")
        m.files["user.dart"] = fill(
            """
            import 'package:json_annotation/json_annotation.dart';

            part 'user.g.dart';

            @JsonSerializable()
            class User {
              const User({required this.id, required this.<<f>>, required this.email});

              factory User.fromJson(Map<String, dynamic> json) => _$UserFromJson(json);

              final String id;
              final String <<f>>;
              final String email;
            }
            """,
            f=f.camel,
        )
        m.files["user.g.dart"] = fill(
            """
            // GENERATED CODE - DO NOT MODIFY BY HAND

            part of 'user.dart';

            User _$UserFromJson(Map<String, dynamic> json) => User(
                  id: json['id'] as String,
                  <<f>>: json['<<f>>'] as String,
                  email: json['email'] as String,
                );
            """,
            f=f.camel,
        )
        m.imports.append("import 'user.dart';")
        m.exports.append("user.dart")
    else:
        m.ty, m.decode = "Map<String, dynamic>", "<<v>>"
    return m


def _kt_class(f, pkg, annotation="", head=""):
    return head + fill(
        """
        package <<pkg>>
        <<imp>>
        <<ann>>data class User(
            val id: String,
            val <<f>>: String,
            val email: String,
        )
        """,
        pkg=pkg, f=f.camel, ann=annotation + "\n" if annotation else "",
        imp="\nimport kotlinx.serialization.Serializable\n" if annotation else "",
    )


def _kotlin(contract, f, note):
    m = Model(ty="User", decode="Gson().fromJson(<<v>>, User::class.java)")
    if contract == "declared-type":
        m.deps.append("gson")
        m.files[KT_API + "User.kt"] = _kt_class(f, "com.sibyl.api")
        m.imports.append("import com.google.gson.Gson")
    elif contract == "generated":
        m.deps.append("gson")
        m.files[KT_API + "generated/User.kt"] = _kt_class(
            f, "com.sibyl.api.generated", head=f"// {note}\n")
        m.imports += ["import com.google.gson.Gson", "import com.sibyl.api.generated.User"]
    elif contract == "schema-object":
        m.deps.append("kotlinx-serialization-json")
        m.files[KT_API + "User.kt"] = _kt_class(f, "com.sibyl.api", "@Serializable")
        m.imports.append("import kotlinx.serialization.json.Json")
        m.decode = "Json.decodeFromString<User>(<<v>>)"
    else:
        m.deps.append("org.json")
        m.imports.append("import org.json.JSONObject")
        m.ty, m.decode = "JSONObject", "JSONObject(<<v>>)"
    return m


def _swift(contract, f, note):
    m = Model(ty="User", decode="try JSONDecoder().decode(User.self, from: <<v>>)")
    struct = fill(
        """
        public struct User: Decodable {
            public let id: String
            public let <<f>>: String
            public let email: String
        }
        """,
        f=f.camel,
    )
    if contract == "declared-type":
        m.files["User.swift"] = struct
    elif contract == "generated":
        m.files["Generated/User.swift"] = f"// {note}\n// DO NOT EDIT.\n\n" + struct
    elif contract == "schema-object":
        m.files["User.swift"] = fill(
            """
            public struct User: Decodable {
                public let id: String
                public let displayName: String
                public let email: String

                enum CodingKeys: String, CodingKey {
                    case id
                    case displayName = "<<f>>"
                    case email
                }
            }
            """,
            f=f.camel,
        )
    else:
        m.ty = "[String: Any]"
        m.decode = "(try JSONSerialization.jsonObject(with: <<v>>)) as? [String: Any] ?? [:]"
    return m


def _python(contract, f, note):
    m = Model(ty="User", decode="User.model_validate(<<v>>)")
    pyd = fill(
        """
        from pydantic import BaseModel, ConfigDict, Field


        class User(BaseModel):
            model_config = ConfigDict(populate_by_name=True)

            id: str
            <<snake>>: str = Field(alias="<<camel>>")
            email: str
        """,
        snake=f.snake, camel=f.camel,
    )
    if contract == "declared-type":
        m.files["models.py"] = fill(
            """
            from dataclasses import dataclass


            @dataclass(frozen=True)
            class User:
                id: str
                <<snake>>: str
                email: str

                @classmethod
                def from_json(cls, data: dict) -> "User":
                    return cls(id=data["id"], <<snake>>=data["<<camel>>"], email=data["email"])
            """,
            snake=f.snake, camel=f.camel,
        )
        m.imports.append("from .models import User")
        m.decode = "User.from_json(<<v>>)"
    elif contract == "generated":
        m.deps.append("pydantic")
        m.files["generated/__init__.py"] = ""
        m.files["generated/models.py"] = f"# {note}\n" + pyd
        m.imports.append("from .generated.models import User")
    elif contract == "schema-object":
        m.deps.append("pydantic")
        m.files["models.py"] = pyd
        m.imports.append("from .models import User")
    else:
        m.ty, m.decode = "dict", "<<v>>"
    return m
