import { credentials } from "@grpc/grpc-js";
import { GreeterClient } from "./generated/greeter_grpc_pb";
import { HelloRequest } from "./generated/greeter_pb";

const client = new GreeterClient("localhost:50051", credentials.createInsecure());

export function sayHello(name: string): void {
  const request = new HelloRequest();
  request.setName(name);
  client.sayHello(request, (err, reply) => {
    if (!err) {
      console.log(reply.getMessage());
    }
  });
}
