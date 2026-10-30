from concurrent import futures

import grpc

import demo_pb2
import demo_pb2_grpc
from grpc_health.v1 import health_pb2
from grpc_health.v1 import health_pb2_grpc


class BaseEmailService(demo_pb2_grpc.EmailServiceServicer):
    def Check(self, request, context):
        return health_pb2.HealthCheckResponse(
            status=health_pb2.HealthCheckResponse.SERVING)

    def Watch(self, request, context):
        return health_pb2.HealthCheckResponse(
            status=health_pb2.HealthCheckResponse.UNIMPLEMENTED)


class DummyEmailService(BaseEmailService):
    def SendOrderConfirmation(self, request, context):
        return demo_pb2.Empty()


def start(dummy_mode):
    server = grpc.server(futures.ThreadPoolExecutor(max_workers=10))
    service = None
    if dummy_mode:
        service = DummyEmailService()
    else:
        raise Exception('non-dummy mode not implemented yet')

    demo_pb2_grpc.add_EmailServiceServicer_to_server(service, server)
    health_pb2_grpc.add_HealthServicer_to_server(service, server)
    server.add_insecure_port('[::]:8080')
    server.start()
