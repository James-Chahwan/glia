import axios from 'axios'

// An axios instance: every call goes through `service({ url, method })`.
const service = axios.create({ baseURL: '', timeout: 99999 })

export default service
