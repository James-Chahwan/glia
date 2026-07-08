<script setup lang="ts">
// IMPORTS extraction probe (Vue SFC). main SFC imports ./greeter.
import { greet } from "./greeter";

const msg = greet("world");
</script>

<template>
  <p>{{ msg }}</p>
</template>
