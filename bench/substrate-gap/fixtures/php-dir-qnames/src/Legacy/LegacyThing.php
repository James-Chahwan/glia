<?php

class LegacyThing
{
    public function go()
    {
        return legacy_helper();
    }
}

function legacy_helper()
{
    return 1;
}
