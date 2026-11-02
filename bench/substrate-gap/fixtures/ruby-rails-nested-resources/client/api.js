// HF.5: the dashboard calls every Rails resource under its account. Each URL
// must pair with the NESTED route (`/api/v1/accounts/:account_id/..`), never
// the flattened `/api/v1/<r>` the route reader emitted before.
import axios from 'axios';

export const listNotifications = (accountId) =>
  axios.get(`/api/v1/accounts/${accountId}/notifications`);

export const deleteNotification = (accountId, id) =>
  axios.delete(`/api/v1/accounts/${accountId}/notifications/${id}`);

export const snoozeNotification = (accountId, id) =>
  axios.post(`/api/v1/accounts/${accountId}/notifications/${id}/snooze`);

export const readAllNotifications = (accountId) =>
  axios.post(`/api/v1/accounts/${accountId}/notifications/read_all`);

export const listMessages = (accountId, id) =>
  axios.get(`/api/v1/accounts/${accountId}/conversations/${id}/messages`);

export const listAssistants = (accountId) =>
  axios.get(`/api/v1/accounts/${accountId}/captain/assistants`);
