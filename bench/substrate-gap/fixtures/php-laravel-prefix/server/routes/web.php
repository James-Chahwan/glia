<?php

use Illuminate\Support\Facades\Route;
use App\Http\Controllers\UserController;
use App\Http\Controllers\HealthController;

// Grouped routes are the default organisation of every non-toy Laravel API.
// The shared segment lives on the GROUP, not on the verb call, so a scanner
// that reads only the verb's first string argument emits `GET /users/{id}`
// and drops `/api/v1` entirely.
Route::prefix('api/v1')->group(function () {
    Route::get('/users/{id}', [UserController::class, 'show']);
    Route::post('/users', [UserController::class, 'store']);

    // A closing brace inside a string literal — the brace matcher must not
    // treat this as the end of the group body. If it does, `/orders` escapes
    // the group and `/health` below wrongly joins it.
    Route::get('/orders', function () { return '} not a brace'; });
});

// Array-options form of the same thing.
Route::group(['prefix' => 'admin', 'middleware' => 'auth'], function () {
    Route::get('/stats', [UserController::class, 'stats']);
});

// CONTROL: declared outside every group, must NOT gain a prefix.
Route::get('/health', [HealthController::class, 'index']);
