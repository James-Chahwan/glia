<template>
  <div class="card">
    <h2>{{ title }}</h2>
    <button @click="refresh">refresh</button>
  </div>
</template>

<script lang="ts">
// The options block: a named export other modules can import.
export function cardTitle(name: string): string {
  return `Card: ${name}`;
}
</script>

<script setup lang="ts">
import { ref } from "vue";
import { formatName } from "./format";

const title = ref("");

function refresh(): void {
  title.value = cardTitle(formatName("ada"));
}
</script>

<style scoped>
.card { padding: 1rem; }
</style>
