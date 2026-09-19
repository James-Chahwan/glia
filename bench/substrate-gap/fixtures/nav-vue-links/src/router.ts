import { createRouter, createWebHistory } from 'vue-router';
import Home from './Home.vue';
import Cart from './Cart.vue';

export default createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', component: Home },
    { path: '/cart', component: Cart },
    { path: '/users/:id', component: () => import('./Cart.vue') },
    { path: '/orders', component: Cart },
  ],
});
