/* The test server, which only a build with the feature `sim` compiles. No other C
   file of `src/` names a `UA_Server`, so a production build cannot reach its
   `setDefaultConfig`. */

/* The headers of the copy have unused parameters. Any other warning in them fails the
   build. */
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-parameter"
#include <open62541/client.h>
#include <open62541/server_config_default.h>
#pragma GCC diagnostic pop

#include <string.h>

#include "shim.h"

/* Gives a server on `el` with the minimal config, the one server URL
 * `opc.tcp://{host}:{port}`, room for `sessions` sessions and secure channels, and at
 * most `chunks` chunks in a message, or NULL on a failure. */
UA_Server *shim_server_new(UA_EventLoop *el, UA_UInt16 port, const char *host,
                           UA_UInt16 sessions, UA_UInt32 chunks) {
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
    config.tcpMaxChunks = chunks;
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

/* Adds to `server` the variable `node`, which holds `value` of the kind `kind`, under
 * the Objects folder. */
UA_StatusCode shim_server_add_scalar(UA_Server *server, shim_node node,
                                     const shim_scalar *value, UA_UInt16 kind) {
    UA_VariableAttributes attributes = UA_VariableAttributes_default;
    UA_Variant_setScalar(&attributes.value, (void *)(uintptr_t)value, &UA_TYPES[kind]);
    attributes.dataType = UA_TYPES[kind].typeId;
    attributes.accessLevel = UA_ACCESSLEVELMASK_READ;
    return UA_Server_addVariableNode(
        server, shim_node_id(node),
        UA_NODEID_NUMERIC(0, UA_NS0ID_OBJECTSFOLDER),
        UA_NODEID_NUMERIC(0, UA_NS0ID_ORGANIZES),
        UA_QUALIFIEDNAME(node.namespace, "value"),
        UA_NODEID_NUMERIC(0, UA_NS0ID_BASEDATAVARIABLETYPE), attributes, NULL, NULL);
}

/* Adds to `server` the variable `node`, which holds an array of two Doubles, under the
 * Objects folder. */
UA_StatusCode shim_server_add_doubles(UA_Server *server, shim_node node) {
    UA_Double doubles[2] = {1.5, 2.5};
    UA_VariableAttributes attributes = UA_VariableAttributes_default;
    UA_Variant_setArray(&attributes.value, doubles, 2, &UA_TYPES[UA_TYPES_DOUBLE]);
    attributes.dataType = UA_TYPES[UA_TYPES_DOUBLE].typeId;
    attributes.accessLevel = UA_ACCESSLEVELMASK_READ;
    return UA_Server_addVariableNode(
        server, shim_node_id(node),
        UA_NODEID_NUMERIC(0, UA_NS0ID_OBJECTSFOLDER),
        UA_NODEID_NUMERIC(0, UA_NS0ID_ORGANIZES),
        UA_QUALIFIEDNAME(node.namespace, "doubles"),
        UA_NODEID_NUMERIC(0, UA_NS0ID_BASEDATAVARIABLETYPE), attributes, NULL, NULL);
}

/* Adds to `server` the variable `node`, which holds the String `text`, under the
 * Objects folder. */
UA_StatusCode shim_server_add_string(UA_Server *server, shim_node node) {
    UA_String text = UA_STRING("text");
    UA_VariableAttributes attributes = UA_VariableAttributes_default;
    UA_Variant_setScalar(&attributes.value, &text, &UA_TYPES[UA_TYPES_STRING]);
    attributes.dataType = UA_TYPES[UA_TYPES_STRING].typeId;
    attributes.accessLevel = UA_ACCESSLEVELMASK_READ;
    return UA_Server_addVariableNode(
        server, shim_node_id(node),
        UA_NODEID_NUMERIC(0, UA_NS0ID_OBJECTSFOLDER),
        UA_NODEID_NUMERIC(0, UA_NS0ID_ORGANIZES),
        UA_QUALIFIEDNAME(node.namespace, "string"),
        UA_NODEID_NUMERIC(0, UA_NS0ID_BASEDATAVARIABLETYPE), attributes, NULL, NULL);
}

/* Adds to `server` the variable `node`, which holds a ByteString of `size` zero
 * bytes, under the Objects folder. */
UA_StatusCode shim_server_add_bytes(UA_Server *server, shim_node node, size_t size) {
    UA_ByteString bytes;
    UA_StatusCode status = UA_ByteString_allocBuffer(&bytes, size);
    if(status != UA_STATUSCODE_GOOD)
        return status;
    memset(bytes.data, 0, size);
    UA_VariableAttributes attributes = UA_VariableAttributes_default;
    UA_Variant_setScalar(&attributes.value, &bytes, &UA_TYPES[UA_TYPES_BYTESTRING]);
    attributes.dataType = UA_TYPES[UA_TYPES_BYTESTRING].typeId;
    attributes.accessLevel = UA_ACCESSLEVELMASK_READ;
    status = UA_Server_addVariableNode(
        server, shim_node_id(node),
        UA_NODEID_NUMERIC(0, UA_NS0ID_OBJECTSFOLDER),
        UA_NODEID_NUMERIC(0, UA_NS0ID_ORGANIZES),
        UA_QUALIFIEDNAME(node.namespace, "bytes"),
        UA_NODEID_NUMERIC(0, UA_NS0ID_BASEDATAVARIABLETYPE), attributes, NULL, NULL);
    UA_ByteString_clear(&bytes);
    return status;
}

/* Sets the buffer size, the MaxMessageSize, and the MaxChunkCount of each later channel
 * of `server`, each 0 for the default of open62541. */
void shim_server_limits(UA_Server *server, UA_UInt32 buffer, UA_UInt32 message,
                        UA_UInt32 chunks) {
    UA_ServerConfig *config = UA_Server_getConfig(server);
    config->tcpBufSize = buffer;
    config->tcpMaxMsgSize = message;
    config->tcpMaxChunks = chunks;
}

/* Sets the MaxNodesPerRead of `server`, 0 for none. */
void shim_server_reads(UA_Server *server, UA_UInt32 most) {
    UA_Server_getConfig(server)->maxNodesPerRead = most;
}

/* Sets the MaxNodesPerBrowse and the MaxNodesPerTranslateBrowsePathsToNodeIds of
 * `server`, each 0 for none. */
void shim_server_browses(UA_Server *server, UA_UInt32 browse, UA_UInt32 translate) {
    UA_ServerConfig *config = UA_Server_getConfig(server);
    config->maxNodesPerBrowse = browse;
    config->maxNodesPerTranslateBrowsePathsToNodeIds = translate;
}

/* A logger that counts the Browse requests of its server, and passes each message to
 * the logger before it. */
typedef struct {
    UA_Logger logger;
    const UA_Logger *next;
    size_t browses;
} shim_counter;

static void count_message(void *context, UA_LogLevel level, UA_LogCategory category,
                          const char *msg, va_list args) {
    shim_counter *counter = (shim_counter *)context;
    /* The end of the format of the debug message of open62541 for each Browse
     * request, from UA_LOG_SESSION_INTERNAL. */
    static const char browse[] = "| Processing BrowseRequest%.0s";
    size_t length = strlen(msg);
    if(length >= sizeof(browse) - 1 &&
       strcmp(msg + length - (sizeof(browse) - 1), browse) == 0)
        counter->browses++;
    counter->next->log(counter->next->context, level, category, msg, args);
}

static void count_clear(UA_Logger *logger) { UA_free(logger->context); }

/* Counts each Browse request that `server` processes from now, and gives the count,
 * which lives until the delete of `server`, or NULL when out of memory. */
const size_t *shim_server_count_browses(UA_Server *server) {
    shim_counter *counter = (shim_counter *)UA_calloc(1, sizeof(*counter));
    if(!counter)
        return NULL;
    UA_ServerConfig *config = UA_Server_getConfig(server);
    counter->next = config->logging;
    counter->logger.log = count_message;
    counter->logger.context = counter;
    counter->logger.clear = count_clear;
    config->logging = &counter->logger;
    return &counter->browses;
}

/* Sets the receive buffer size that `client` gives in its next HEL. */
void shim_client_receive(UA_Client *client, UA_UInt32 size) {
    UA_Client_getConfig(client)->localConnectionConfig.recvBufferSize = size;
}

/* The node context of a variable of `shim_server_add_source`, as the first field of
 * the context that Rust holds. */
typedef struct shim_source shim_source;
struct shim_source {
    /* Gives the value, its kind, and the status of a read of the variable. */
    void (*read)(shim_source *source, shim_scalar *value, UA_UInt16 *kind,
                 UA_StatusCode *status);
};

static UA_StatusCode shim_source_read(UA_Server *server, const UA_NodeId *sessionId,
                                      void *sessionContext, const UA_NodeId *nodeId,
                                      void *nodeContext,
                                      UA_Boolean includeSourceTimeStamp,
                                      const UA_NumericRange *range,
                                      UA_DataValue *value) {
    (void)server;
    (void)sessionId;
    (void)sessionContext;
    (void)nodeId;
    (void)includeSourceTimeStamp;
    /* The client of the crate reads no range. */
    (void)range;
    value->hasStatus = true;
    shim_source *source = nodeContext;
    shim_scalar read;
    UA_UInt16 kind;
    source->read(source, &read, &kind, &value->status);
    UA_StatusCode status =
        UA_Variant_setScalarCopy(&value->value, &read, &UA_TYPES[kind]);
    value->hasValue = status == UA_STATUSCODE_GOOD;
    return status;
}

/* Adds to `server` the scalar variable `node` of the DataType BaseDataType under the
 * Objects folder, whose reads `source` answers. `source` lives until the server is
 * deleted. */
UA_StatusCode shim_server_add_source(UA_Server *server, shim_node node,
                                     shim_source *source) {
    UA_VariableAttributes attributes = UA_VariableAttributes_default;
    attributes.dataType = UA_NODEID_NUMERIC(0, UA_NS0ID_BASEDATATYPE);
    attributes.valueRank = UA_VALUERANK_SCALAR;
    attributes.accessLevel = UA_ACCESSLEVELMASK_READ;
    UA_CallbackValueSource callbacks = {.read = shim_source_read, .write = NULL};
    return UA_Server_addCallbackValueSourceVariableNode(
        server, shim_node_id(node),
        UA_NODEID_NUMERIC(0, UA_NS0ID_OBJECTSFOLDER),
        UA_NODEID_NUMERIC(0, UA_NS0ID_ORGANIZES),
        UA_QUALIFIEDNAME(node.namespace, "source"),
        UA_NODEID_NUMERIC(0, UA_NS0ID_BASEDATAVARIABLETYPE), attributes, callbacks,
        source, NULL);
}

/* Makes `source` answer each later read of the variable `node` of `server`, which
 * open62541 adds, as `shim_server_add_source` does. `source` lives until the server
 * is deleted. */
UA_StatusCode shim_server_set_source(UA_Server *server, shim_node node,
                                     shim_source *source) {
    UA_NodeId id = shim_node_id(node);
    UA_StatusCode status = UA_Server_setNodeContext(server, id, source);
    if(status != UA_STATUSCODE_GOOD)
        return status;
    UA_CallbackValueSource callbacks = {.read = shim_source_read, .write = NULL};
    return UA_Server_setVariableNode_callbackValueSource(server, id, callbacks);
}

/* Sets the most session timeout of `server`, in ms. */
void shim_server_session_timeout(UA_Server *server, UA_UInt32 ms) {
    UA_Server_getConfig(server)->maxSessionTimeout = ms;
}

/* Gives in `id` the number of the DataType of the variable `node` of `server`.
 * Returns `UA_STATUSCODE_BADTYPEMISMATCH` when the DataType is not a number of
 * namespace 0. */
UA_StatusCode shim_server_data_type(UA_Server *server, shim_node node, UA_UInt32 *id) {
    UA_NodeId type;
    UA_StatusCode status = UA_Server_readDataType(server, shim_node_id(node), &type);
    if(status != UA_STATUSCODE_GOOD)
        return status;
    if(type.namespaceIndex != 0 || type.identifierType != UA_NODEIDTYPE_NUMERIC)
        status = UA_STATUSCODE_BADTYPEMISMATCH;
    else
        *id = type.identifier.numeric;
    UA_NodeId_clear(&type);
    return status;
}

/* Gives the text `name` in the namespace of `node`, which borrows `name`. */
static UA_QualifiedName name_of(shim_node node, const char *name) {
    return UA_QUALIFIEDNAME(node.namespace, (char *)(uintptr_t)name);
}

/* Adds to `server` the folder `node` with the browse name `name` and the display name
 * `display`, under `parent` by the reference of the number `reference`. */
UA_StatusCode shim_server_add_folder(UA_Server *server, shim_node node,
                                     shim_node parent, UA_UInt32 reference,
                                     const char *name, const char *display) {
    UA_ObjectAttributes attributes = UA_ObjectAttributes_default;
    attributes.displayName = UA_LOCALIZEDTEXT("", (char *)(uintptr_t)display);
    return UA_Server_addObjectNode(
        server, shim_node_id(node), shim_node_id(parent),
        UA_NODEID_NUMERIC(0, reference), name_of(node, name),
        UA_NODEID_NUMERIC(0, UA_NS0ID_FOLDERTYPE), attributes, NULL, NULL);
}

/* Adds to `server` the folder of the GUID 1-2-3-4 in namespace 1, with the browse name
 * `guid`, under `parent`. */
UA_StatusCode shim_server_add_guid(UA_Server *server, shim_node parent) {
    UA_Guid guid = {1, 2, 3, {4, 0, 0, 0, 0, 0, 0, 0}};
    UA_ObjectAttributes attributes = UA_ObjectAttributes_default;
    attributes.displayName = UA_LOCALIZEDTEXT("", "guid");
    return UA_Server_addObjectNode(
        server, UA_NODEID_GUID(1, guid), shim_node_id(parent),
        UA_NODEID_NUMERIC(0, UA_NS0ID_ORGANIZES), UA_QUALIFIEDNAME(1, "guid"),
        UA_NODEID_NUMERIC(0, UA_NS0ID_FOLDERTYPE), attributes, NULL, NULL);
}

/* Adds to `server` the variable `node` with no value, or two Doubles for the rank 1,
 * the browse and display name `name`, the DataType `type`, the ValueRank `rank`, and
 * the AccessLevel and UserAccessLevel `access`, under `parent` by the reference of the
 * number `reference`. */
UA_StatusCode shim_server_add_variable(UA_Server *server, shim_node node,
                                       shim_node parent, UA_UInt32 reference,
                                       const char *name, shim_node type, UA_Int32 rank,
                                       UA_Byte access) {
    UA_VariableAttributes attributes = UA_VariableAttributes_default;
    attributes.displayName = UA_LOCALIZEDTEXT("", (char *)(uintptr_t)name);
    attributes.dataType = shim_node_id(type);
    attributes.valueRank = rank;
    UA_Double doubles[2] = {1.5, 2.5};
    UA_UInt32 dimensions[1] = {0};
    if(rank == 1) {
        UA_Variant_setArray(&attributes.value, doubles, 2, &UA_TYPES[UA_TYPES_DOUBLE]);
        attributes.arrayDimensions = dimensions;
        attributes.arrayDimensionsSize = 1;
    }
    attributes.accessLevel = access;
    attributes.userAccessLevel = access;
    return UA_Server_addVariableNode(
        server, shim_node_id(node), shim_node_id(parent),
        UA_NODEID_NUMERIC(0, reference), name_of(node, name),
        UA_NODEID_NUMERIC(0, UA_NS0ID_BASEDATAVARIABLETYPE), attributes, NULL, NULL);
}

/* Adds to `server` the data type `node` with the browse name `name`, a subtype of
 * `parent`. */
UA_StatusCode shim_server_add_data_type(UA_Server *server, shim_node node,
                                        shim_node parent, const char *name) {
    UA_DataTypeAttributes attributes = UA_DataTypeAttributes_default;
    attributes.displayName = UA_LOCALIZEDTEXT("", (char *)(uintptr_t)name);
    return UA_Server_addDataTypeNode(server, shim_node_id(node), shim_node_id(parent),
                                     UA_NODEID_NUMERIC(0, UA_NS0ID_HASSUBTYPE),
                                     name_of(node, name), attributes, NULL, NULL);
}

/* Adds to `server` the property `0:EngineeringUnits` `node` of the variable
 * `variable`, whose value is the EUInformation of the namespaceUri `uri`, the display
 * name `text`, and the unitId `code`. */
UA_StatusCode shim_server_add_unit(UA_Server *server, shim_node node,
                                   shim_node variable, const char *uri,
                                   const char *text, UA_Int32 code) {
    UA_EUInformation unit;
    UA_EUInformation_init(&unit);
    unit.namespaceUri = UA_STRING((char *)(uintptr_t)uri);
    unit.unitId = code;
    unit.displayName = UA_LOCALIZEDTEXT("", (char *)(uintptr_t)text);
    UA_VariableAttributes attributes = UA_VariableAttributes_default;
    attributes.displayName = UA_LOCALIZEDTEXT("", "EngineeringUnits");
    attributes.dataType = UA_NODEID_NUMERIC(0, UA_NS0ID_BASEDATATYPE);
    attributes.accessLevel = UA_ACCESSLEVELMASK_READ;
    UA_Variant_setScalar(&attributes.value, &unit, &UA_TYPES[UA_TYPES_EUINFORMATION]);
    return UA_Server_addVariableNode(
        server, shim_node_id(node), shim_node_id(variable),
        UA_NODEID_NUMERIC(0, UA_NS0ID_HASPROPERTY),
        UA_QUALIFIEDNAME(0, "EngineeringUnits"),
        UA_NODEID_NUMERIC(0, UA_NS0ID_PROPERTYTYPE), attributes, NULL, NULL);
}

/* Adds to `server` a reference of the number `reference` from `source` to `target`. */
UA_StatusCode shim_server_add_reference(UA_Server *server, shim_node source,
                                        UA_UInt32 reference, shim_node target) {
    UA_ExpandedNodeId to = UA_EXPANDEDNODEID_NULL;
    to.nodeId = shim_node_id(target);
    return UA_Server_addReference(server, shim_node_id(source),
                                  UA_NODEID_NUMERIC(0, reference), to, true);
}

/* Deletes from `server` the reference of the number `reference` from `source` to
 * `target`, in both directions. */
UA_StatusCode shim_server_delete_reference(UA_Server *server, shim_node source,
                                           UA_UInt32 reference, shim_node target) {
    UA_ExpandedNodeId to = UA_EXPANDEDNODEID_NULL;
    to.nodeId = shim_node_id(target);
    return UA_Server_deleteReference(server, shim_node_id(source),
                                     UA_NODEID_NUMERIC(0, reference), true, to, true);
}

/* Adds to `server` the view `node` with the browse and display name `name`, under
 * `parent` by Organizes. */
UA_StatusCode shim_server_add_view(UA_Server *server, shim_node node, shim_node parent,
                                   const char *name) {
    UA_ViewAttributes attributes = UA_ViewAttributes_default;
    attributes.displayName = UA_LOCALIZEDTEXT("", (char *)(uintptr_t)name);
    return UA_Server_addViewNode(server, shim_node_id(node), shim_node_id(parent),
                                 UA_NODEID_NUMERIC(0, UA_NS0ID_ORGANIZES),
                                 name_of(node, name), attributes, NULL, NULL);
}

/* Gives the node of the name `name` in namespace 1, which borrows `name`. */
static shim_node named(const char *name) {
    shim_node node = {1, 0, (const UA_Byte *)name, strlen(name)};
    return node;
}

/* Adds to `server` the variable `name` of namespace 1 with the browse and display name
 * `display`, the DataType `type`, the scalar `value` of `kind`, and the AccessLevel and
 * UserAccessLevel read, under `parent` by HasComponent. */
static UA_StatusCode add_value(UA_Server *server, const char *name, const char *parent,
                               const char *display, UA_NodeId type, const void *value,
                               const UA_DataType *kind) {
    UA_VariableAttributes attributes = UA_VariableAttributes_default;
    attributes.displayName = UA_LOCALIZEDTEXT("", (char *)(uintptr_t)display);
    attributes.dataType = type;
    attributes.valueRank = UA_VALUERANK_SCALAR;
    attributes.accessLevel = UA_ACCESSLEVELMASK_READ;
    attributes.userAccessLevel = UA_ACCESSLEVELMASK_READ;
    UA_Variant_setScalar(&attributes.value, (void *)(uintptr_t)value, kind);
    return UA_Server_addVariableNode(
        server, shim_node_id(named(name)), shim_node_id(named(parent)),
        UA_NODEID_NUMERIC(0, UA_NS0ID_HASCOMPONENT), name_of(named(name), display),
        UA_NODEID_NUMERIC(0, UA_NS0ID_BASEDATAVARIABLETYPE), attributes, NULL, NULL);
}

#define TRY(call)                                                                    \
    do {                                                                             \
        UA_StatusCode status = (call);                                               \
        if(status != UA_STATUSCODE_GOOD)                                             \
            return status;                                                           \
    } while(0)

/* Adds to `server` the tree of `sim::serve`, and caps each page of a Browse at 8
 * references. */
UA_StatusCode shim_server_add_tree(UA_Server *server) {
    UA_Server_getConfig(server)->maxReferencesPerNode = 8;
    const shim_node objects = {0, UA_NS0ID_OBJECTSFOLDER, NULL, 0};
    TRY(shim_server_add_folder(server, named("Plant"), objects, UA_NS0ID_ORGANIZES,
                               "Plant", "Plant"));
    TRY(shim_server_add_folder(server, named("Plant.Types"), named("Plant"),
                               UA_NS0ID_ORGANIZES, "Types", "Types"));
    UA_Boolean boolean = true;
    UA_SByte sbyte = -8;
    UA_Byte byte = 8;
    UA_Int16 int16 = -16;
    UA_UInt16 uint16 = 16;
    UA_Int32 int32 = -32;
    UA_UInt32 uint32 = 32;
    UA_Int64 int64 = -64;
    UA_UInt64 uint64 = 64;
    UA_Float float_ = 0.5f;
    UA_Double double_ = 0.25;
    UA_String string = UA_STRING_STATIC("text");
    UA_DateTime time = UA_DATETIME_UNIX_EPOCH;
    UA_Guid guid = {1, 2, 3, {4, 0, 0, 0, 0, 0, 0, 0}};
    UA_Byte two[2] = {1, 2};
    UA_ByteString bytes = {2, two};
    const struct {
        const char *name;
        size_t kind;
        const void *value;
    } types[] = {
        {"Plant.Types.Boolean", UA_TYPES_BOOLEAN, &boolean},
        {"Plant.Types.SByte", UA_TYPES_SBYTE, &sbyte},
        {"Plant.Types.Byte", UA_TYPES_BYTE, &byte},
        {"Plant.Types.Int16", UA_TYPES_INT16, &int16},
        {"Plant.Types.UInt16", UA_TYPES_UINT16, &uint16},
        {"Plant.Types.Int32", UA_TYPES_INT32, &int32},
        {"Plant.Types.UInt32", UA_TYPES_UINT32, &uint32},
        {"Plant.Types.Int64", UA_TYPES_INT64, &int64},
        {"Plant.Types.UInt64", UA_TYPES_UINT64, &uint64},
        {"Plant.Types.Float", UA_TYPES_FLOAT, &float_},
        {"Plant.Types.Double", UA_TYPES_DOUBLE, &double_},
        {"Plant.Types.String", UA_TYPES_STRING, &string},
        {"Plant.Types.DateTime", UA_TYPES_DATETIME, &time},
        {"Plant.Types.Guid", UA_TYPES_GUID, &guid},
        {"Plant.Types.ByteString", UA_TYPES_BYTESTRING, &bytes},
    };
    for(size_t i = 0; i < sizeof(types) / sizeof(types[0]); i++) {
        const char *name = types[i].name;
        const UA_DataType *kind = &UA_TYPES[types[i].kind];
        TRY(add_value(server, name, "Plant.Types", name + strlen("Plant.Types."),
                      kind->typeId, types[i].value, kind));
    }
    UA_ObjectAttributes line = UA_ObjectAttributes_default;
    line.displayName = UA_LOCALIZEDTEXT("", "Line");
    TRY(UA_Server_addObjectNode(
        server, shim_node_id(named("Plant.Line")), shim_node_id(named("Plant")),
        UA_NODEID_NUMERIC(0, UA_NS0ID_HASCOMPONENT), UA_QUALIFIEDNAME(1, "Line"),
        UA_NODEID_NUMERIC(0, UA_NS0ID_BASEOBJECTTYPE), line, NULL, NULL));
    TRY(shim_server_add_reference(server, named("Plant.Line"), UA_NS0ID_ORGANIZES,
                                  named("Plant")));
    const UA_NodeId double_id = UA_TYPES[UA_TYPES_DOUBLE].typeId;
    UA_Double temperature = 21.5;
    TRY(add_value(server, "Plant.Line.Temperature", "Plant.Line", "Temperature",
                  double_id, &temperature, &UA_TYPES[UA_TYPES_DOUBLE]));
    /* UN/CEFACT `CEL`. */
    TRY(shim_server_add_unit(server, named("Plant.Line.Temperature.EngineeringUnits"),
                             named("Plant.Line.Temperature"),
                             "http://www.opcfoundation.org/UA/units/un/cefact",
                             "\xC2\xB0"
                             "C",
                             0x0043454C));
    /* The namespace 0 of the build has no node of these types. */
    const shim_node base = {0, UA_NS0ID_BASEDATATYPE, NULL, 0};
    const shim_node number = {0, UA_NS0ID_DOUBLE, NULL, 0};
    const shim_node enumeration = {0, UA_NS0ID_ENUMERATION, NULL, 0};
    const shim_node structure = {0, UA_NS0ID_STRUCTURE, NULL, 0};
    TRY(shim_server_add_data_type(server, number, base, "Double"));
    TRY(shim_server_add_data_type(server, enumeration, base, "Enumeration"));
    TRY(shim_server_add_data_type(server, structure, base, "Structure"));
    TRY(shim_server_add_data_type(server, named("Speed"), number, "Speed"));
    TRY(shim_server_add_data_type(server, named("Mode"), enumeration, "Mode"));
    TRY(shim_server_add_data_type(server, named("Recipe"), structure, "Recipe"));
    UA_Double speed = 1.25;
    TRY(add_value(server, "Plant.Line.Speed", "Plant.Line", "Speed",
                  shim_node_id(named("Speed")), &speed, &UA_TYPES[UA_TYPES_DOUBLE]));
    /* open62541 refuses an Int32 value of a subtype of Enumeration. */
    TRY(shim_server_add_variable(server, named("Plant.Line.Mode"), named("Plant.Line"),
                                 UA_NS0ID_HASCOMPONENT, "Mode", named("Mode"), -1,
                                 UA_ACCESSLEVELMASK_READ));
    TRY(shim_server_add_variable(server, named("Plant.Line.Profile"),
                                 named("Plant.Line"), UA_NS0ID_HASCOMPONENT,
                                 "Profile", number, 1, UA_ACCESSLEVELMASK_READ));
    TRY(shim_server_add_variable(server, named("Plant.Line.Locked"),
                                 named("Plant.Line"), UA_NS0ID_HASCOMPONENT, "Locked",
                                 number, -1, 0));
    UA_ExtensionObject recipe;
    UA_ExtensionObject_init(&recipe);
    recipe.encoding = UA_EXTENSIONOBJECT_ENCODED_BYTESTRING;
    recipe.content.encoded.typeId = shim_node_id(named("Recipe"));
    recipe.content.encoded.body = bytes;
    TRY(add_value(server, "Plant.Line.Recipe", "Plant.Line", "Recipe",
                  shim_node_id(named("Recipe")), &recipe,
                  &UA_TYPES[UA_TYPES_EXTENSIONOBJECT]));
    TRY(shim_server_add_folder(server, named("Plant.Odd"), named("Plant"),
                               UA_NS0ID_ORGANIZES, "Odd", "Odd"));
    UA_ObjectAttributes key = UA_ObjectAttributes_default;
    key.displayName = UA_LOCALIZEDTEXT("", "Guid");
    TRY(UA_Server_addObjectNode(
        server, UA_NODEID_GUID(1, ((UA_Guid){5, 6, 7, {8, 0, 0, 0, 0, 0, 0, 0}})),
        shim_node_id(named("Plant.Odd")), UA_NODEID_NUMERIC(0, UA_NS0ID_ORGANIZES),
        UA_QUALIFIEDNAME(1, "Guid"), UA_NODEID_NUMERIC(0, UA_NS0ID_FOLDERTYPE), key,
        NULL, NULL));
    UA_Double level = 2.5;
    TRY(add_value(server, "Plant.Odd.Level", "Plant.Odd", "Level", double_id, &level,
                  &UA_TYPES[UA_TYPES_DOUBLE]));
    TRY(shim_server_add_reference(server, named("Plant.Odd"), UA_NS0ID_ORGANIZES,
                                  named("Plant.Odd.Level")));
    UA_Double twin = 3.5;
    TRY(add_value(server, "Plant.Odd.Twin", "Plant.Odd", "Level", double_id, &twin,
                  &UA_TYPES[UA_TYPES_DOUBLE]));
    return UA_STATUSCODE_GOOD;
}
