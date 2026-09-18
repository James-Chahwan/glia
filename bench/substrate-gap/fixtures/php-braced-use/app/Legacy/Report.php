<?php
namespace App\Legacy;

use App\Models\User;

class Report {
    public function run($id) {
        $u = new User();
        return $u->find($id);
    }
}
