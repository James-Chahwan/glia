<?php

namespace App\Braced {
    class Report
    {
        public function build()
        {
            return $this->sum();
        }

        private function sum()
        {
            return 2;
        }
    }
}
