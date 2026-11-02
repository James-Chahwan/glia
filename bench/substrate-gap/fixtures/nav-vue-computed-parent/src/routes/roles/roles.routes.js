import { appURL } from '../../helper';
import RolesWrapper from './RolesWrapper.vue';
import RolesList from './RolesList.vue';

export default {
  routes: [
    {
      path: appURL('teams/:teamId/roles'),
      component: RolesWrapper,
      children: [
        { path: '', redirect: 'list' },
        { path: 'list', component: RolesList },
      ],
    },
  ],
};
