package com.example.client;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import okhttp3.OkHttpClient;
import okhttp3.Request;
import org.apache.http.client.methods.HttpDelete;

public class Clients {
    private final HttpClient http = HttpClient.newHttpClient();
    private final OkHttpClient ok = new OkHttpClient();

    public String jdk() throws Exception {
        HttpRequest req = HttpRequest.newBuilder().uri(URI.create("http://users-svc/users")).GET().build();
        return http.send(req, HttpResponse.BodyHandlers.ofString()).body();
    }

    public String jdkPost() throws Exception {
        HttpRequest req = HttpRequest.newBuilder(URI.create("http://orders-svc/orders"))
            .POST(HttpRequest.BodyPublishers.ofString("{}")).build();
        return http.send(req, HttpResponse.BodyHandlers.ofString()).body();
    }

    public String okhttp() throws Exception {
        Request request = new Request.Builder().url("http://users-svc/accounts").build();
        return ok.newCall(request).execute().body().string();
    }

    public void apache() throws Exception {
        HttpDelete del = new HttpDelete("http://users-svc/invoices");
    }
}
