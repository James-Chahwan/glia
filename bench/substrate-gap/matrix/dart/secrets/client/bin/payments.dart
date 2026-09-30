import 'dart:io';

String stripeSecretKey() => Platform.environment['STRIPE_SECRET_KEY'] ?? '';
