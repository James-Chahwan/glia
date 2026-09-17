#include <curl/curl.h>

int list_users(void) {
    CURL *curl = curl_easy_init();
    curl_easy_setopt(curl, CURLOPT_URL, "http://api/users");
    int rc = curl_easy_perform(curl);
    curl_easy_cleanup(curl);
    return rc;
}

int create_user(const char *json) {
    CURL *curl = curl_easy_init();
    curl_easy_setopt(curl, CURLOPT_URL, "http://api/users");
    curl_easy_setopt(curl, CURLOPT_POSTFIELDS, json);
    int rc = curl_easy_perform(curl);
    curl_easy_cleanup(curl);
    return rc;
}
