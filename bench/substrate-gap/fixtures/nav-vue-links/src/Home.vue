<template>
  <router-link to="/cart">Cart</router-link>
  <RouterLink :to="`/users/${u.id}`">U</RouterLink>
</template>

<script setup lang="ts">
import { useRouter } from 'vue-router';
const router = useRouter();
const u = { id: 1 };
function go() {
  router.push('/cart');
  router.push({ path: '/orders' });
  router.push({ name: 'cart' });
}
</script>
