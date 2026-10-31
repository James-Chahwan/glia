import 'dart:io';

const apiKey = String.fromEnvironment('API_KEY');

bool debugOn() => Platform.environment.containsKey('APP_DEBUG');
