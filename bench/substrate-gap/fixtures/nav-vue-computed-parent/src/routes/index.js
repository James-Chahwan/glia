import { createRouter, createWebHistory } from 'vue-router';
import Home from './Home.vue';
import roles from './roles/roles.routes';

export default createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', component: Home },
    {
      path: '/settings',
      component: Home,
      children: [
        { path: '', redirect: 'general' },
        { path: 'general', component: Home },
      ],
    },
    ...roles.routes,
  ],
});
