import { Server, ServerCredentials, ServerUnaryCall, sendUnaryData } from "@grpc/grpc-js";
import { GreeterService } from "./generated/greeter_grpc_pb";
import { HelloReply, HelloRequest } from "./generated/greeter_pb";

function sayHello(
  call: ServerUnaryCall<HelloRequest, HelloReply>,
  callback: sendUnaryData<HelloReply>,
): void {
  const reply = new HelloReply();
  reply.setMessage(`Hello ${call.request.getName()}`);
  callback(null, reply);
}

const server = new Server();
server.addService(GreeterService, { sayHello });
server.bindAsync("0.0.0.0:50051", ServerCredentials.createInsecure(), () => server.start());
