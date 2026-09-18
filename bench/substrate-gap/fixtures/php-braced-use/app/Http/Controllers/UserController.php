<?php
namespace App\Http\Controllers {
use App\Models\User;

class UserController {
    public function show($id) {
        $u = new User();
        return $u->find($id);
    }
}
}
