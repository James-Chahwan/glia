<script setup lang="ts">
// Vue SFC <script setup> calling a backend route via axios.
// Cross-stack: the enclosing function CALLS the /users ENDPOINT; the
// HttpStackResolver should pair it with the Go /users ROUTE in server/.
import axios from "axios";
import { ref } from "vue";

const users = ref<unknown[]>([]);

async function loadUsers(): Promise<void> {
  const res = await axios.get("/users");
  users.value = res.data;
}

async function addUser(body: unknown): Promise<void> {
  await axios.post("/users", body);
}
</script>

<template>
  <button @click="loadUsers">load</button>
  <button @click="addUser({})">add</button>
</template>
