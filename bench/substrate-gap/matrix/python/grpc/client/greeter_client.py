import grpc

import greeter_pb2
import greeter_pb2_grpc


def say_hello(name):
    with grpc.insecure_channel("localhost:50051") as channel:
        stub = greeter_pb2_grpc.GreeterStub(channel)
        reply = stub.SayHello(greeter_pb2.HelloRequest(name=name))
    return reply.message
