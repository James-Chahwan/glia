package com.example.client;

import retrofit2.Call;
import retrofit2.http.Body;
import retrofit2.http.POST;

public interface RepoApi {
    @POST("repos")
    Call<Void> create(@Body Object b);
}
