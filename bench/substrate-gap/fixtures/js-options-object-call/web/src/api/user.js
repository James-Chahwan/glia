import service from '@/utils/request'

export const login = (data) => {
  return service({
    url: '/base/login',
    method: 'post',
    data: data
  })
}

export const getUserList = (data) => {
  return service({
    url: '/user/getUserList',
    method: 'post',
    data
  })
}

export const getUserInfo = () => {
  return service({
    url: '/user/getUserInfo',
    method: 'get'
  })
}
