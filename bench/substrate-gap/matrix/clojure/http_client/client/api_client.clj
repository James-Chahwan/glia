(ns app.api-client
  (:require [clj-http.client :as client]))

(def base "http://api")

(defn list-users []
  (:body (client/get "http://api/users" {:as :json})))

(defn create-user [user]
  (client/post "http://api/users" {:form-params user :content-type :json}))

(defn user-count [token]
  (let [r (client/get (str base "/users") {:headers {"Authorization" token}})]
    (count (:body r))))
