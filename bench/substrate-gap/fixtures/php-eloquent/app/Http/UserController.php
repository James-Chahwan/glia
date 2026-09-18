<?php
namespace App\Http;

use App\Models\User;
use Carbon\Carbon;
use Illuminate\Support\Facades\DB;
use Illuminate\Support\Facades\Session;

class UserController
{
    public function active() { return User::where('active', 1)->get(); }

    public function audit() { DB::table('audit_log')->insert(['event' => 'viewed']); }

    public function since() { return Carbon::create(2024, 1, 1); }

    public function flash() { return Session::all(); }
}
