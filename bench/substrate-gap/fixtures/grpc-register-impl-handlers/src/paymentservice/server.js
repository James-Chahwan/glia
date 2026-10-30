const path = require('path');
const grpc = require('@grpc/grpc-js');
const protoLoader = require('@grpc/proto-loader');
const charge = require('./charge');

class HipsterShopServer {
  constructor(protoRoot, port) {
    this.port = port;
    this.packages = {
      hipsterShop: this.loadProto(path.join(protoRoot, 'demo.proto')),
      health: this.loadProto(path.join(protoRoot, 'grpc/health/v1/health.proto'))
    };
    this.server = new grpc.Server();
    this.loadAllProtos(protoRoot);
  }

  static ChargeServiceHandler(call, callback) {
    callback(null, charge(call.request));
  }

  static CheckHandler(call, callback) {
    callback(null, { status: 'SERVING' });
  }

  loadProto(file) {
    const def = protoLoader.loadSync(file, { keepCase: true });
    return grpc.loadPackageDefinition(def);
  }

  loadAllProtos(protoRoot) {
    const hipsterShopPackage = this.packages.hipsterShop.hipstershop;
    const healthPackage = this.packages.health.grpc.health.v1;

    this.server.addService(
      hipsterShopPackage.PaymentService.service,
      {
        charge: HipsterShopServer.ChargeServiceHandler.bind(this)
      }
    );

    this.server.addService(
      healthPackage.Health.service,
      {
        check: HipsterShopServer.CheckHandler.bind(this)
      }
    );
  }
}

module.exports = HipsterShopServer;
