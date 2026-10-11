/* The test server, which only a build with the feature `sim` compiles. No other C
   file of `src/` names a `UA_Server`, so a production build cannot reach its
   `setDefaultConfig`. */

/* The headers of the copy have unused parameters. Any other warning in them fails the
   build. */
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-parameter"
#include <open62541/server_config_default.h>
#pragma GCC diagnostic pop

#include <string.h>

/* Gives a server on `el` with the minimal config, the one server URL
 * `opc.tcp://{host}:{port}`, and room for `sessions` sessions and secure channels, or
 * NULL on a failure. */
UA_Server *shim_server_new(UA_EventLoop *el, UA_UInt16 port, const char *host,
                           UA_UInt16 sessions) {
    UA_ServerConfig config;
    memset(&config, 0, sizeof(config));
    /* The loop of `shim_loop_new` owns its logger, which is not const. */
    config.logging = (UA_Logger *)(uintptr_t)el->logger;
    config.eventLoop = el;
    config.externalEventLoop = true;
    if(UA_ServerConfig_setMinimal(&config, port, NULL) != UA_STATUSCODE_GOOD) {
        UA_ServerConfig_clear(&config);
        return NULL;
    }
    config.maxSessions = sessions;
    config.maxSecureChannels = sessions;
    /* The minimal config has the one server URL `opc.tcp://:{port}`. */
    UA_String *url = &config.serverUrls[0];
    UA_String_clear(url);
    if(UA_String_format(url, "opc.tcp://%s:%u", host, (unsigned)port) !=
       UA_STATUSCODE_GOOD) {
        UA_ServerConfig_clear(&config);
        return NULL;
    }
    return UA_Server_newWithConfig(&config);
}

/* Gives discovery URL `index` of `server`, or NULL past the last. */
const UA_String *shim_server_discovery_url(UA_Server *server, size_t index) {
    const UA_ApplicationDescription *description =
        &UA_Server_getConfig(server)->applicationDescription;
    if(index >= description->discoveryUrlsSize)
        return NULL;
    return &description->discoveryUrls[index];
}
