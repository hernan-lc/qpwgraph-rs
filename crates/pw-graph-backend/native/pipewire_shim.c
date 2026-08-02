#define _GNU_SOURCE

#include <errno.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <pipewire/pipewire.h>
#include <spa/utils/dict.h>
#include <spa/utils/hook.h>

#define PW_GRAPH_MAX_NODES 4096
#define PW_GRAPH_MAX_PORTS 16384
#define PW_GRAPH_MAX_LINKS 16384
#define PW_GRAPH_NAME_SIZE 256
#define PW_GRAPH_TYPE_SIZE 64

struct pw_graph_node {
    uint32_t id;
    char name[PW_GRAPH_NAME_SIZE];
};

struct pw_graph_port {
    uint32_t id;
    uint32_t node_id;
    uint32_t direction;
    char name[PW_GRAPH_NAME_SIZE];
    char media_type[PW_GRAPH_TYPE_SIZE];
};

struct pw_graph_link {
    uint32_t id;
    uint32_t output_port;
    uint32_t input_port;
};

struct pw_graph_snapshot {
    uint32_t node_count;
    uint32_t port_count;
    uint32_t link_count;
    struct pw_graph_node nodes[PW_GRAPH_MAX_NODES];
    struct pw_graph_port ports[PW_GRAPH_MAX_PORTS];
    struct pw_graph_link links[PW_GRAPH_MAX_LINKS];
};

struct pw_graph_shim {
    struct pw_thread_loop *thread_loop;
    struct pw_context *context;
    struct pw_core *core;
    struct pw_registry *registry;
    struct spa_hook core_listener;
    struct spa_hook registry_listener;
    int sync_seq;
    bool sync_done;
    int sync_error;
    struct pw_graph_node nodes[PW_GRAPH_MAX_NODES];
    size_t node_count;
    struct pw_graph_port ports[PW_GRAPH_MAX_PORTS];
    size_t port_count;
    struct pw_graph_link links[PW_GRAPH_MAX_LINKS];
    size_t link_count;
};

static void copy_text(char *destination, size_t size, const char *source, const char *fallback)
{
    const char *value = source != NULL && source[0] != '\0' ? source : fallback;
    if (value == NULL) {
        value = "";
    }
    snprintf(destination, size, "%s", value);
}

static uint32_t parse_u32(const char *value, uint32_t fallback)
{
    char *end = NULL;
    unsigned long parsed;
    if (value == NULL || value[0] == '\0') {
        return fallback;
    }
    parsed = strtoul(value, &end, 10);
    if (end == value || *end != '\0' || parsed > UINT32_MAX) {
        return fallback;
    }
    return (uint32_t)parsed;
}

static int node_index(struct pw_graph_shim *shim, uint32_t id)
{
    size_t i;
    for (i = 0; i < shim->node_count; ++i) {
        if (shim->nodes[i].id == id) {
            return (int)i;
        }
    }
    return -1;
}

static int port_index(struct pw_graph_shim *shim, uint32_t id)
{
    size_t i;
    for (i = 0; i < shim->port_count; ++i) {
        if (shim->ports[i].id == id) {
            return (int)i;
        }
    }
    return -1;
}

static int link_index(struct pw_graph_shim *shim, uint32_t id)
{
    size_t i;
    for (i = 0; i < shim->link_count; ++i) {
        if (shim->links[i].id == id) {
            return (int)i;
        }
    }
    return -1;
}

static void remove_node(struct pw_graph_shim *shim, uint32_t id)
{
    int index = node_index(shim, id);
    if (index >= 0) {
        shim->nodes[index] = shim->nodes[shim->node_count - 1];
        --shim->node_count;
    }
}

static void remove_port(struct pw_graph_shim *shim, uint32_t id)
{
    int index = port_index(shim, id);
    if (index >= 0) {
        shim->ports[index] = shim->ports[shim->port_count - 1];
        --shim->port_count;
    }
}

static void remove_link(struct pw_graph_shim *shim, uint32_t id)
{
    int index = link_index(shim, id);
    if (index >= 0) {
        shim->links[index] = shim->links[shim->link_count - 1];
        --shim->link_count;
    }
}

static void on_registry_global(void *data, uint32_t id, uint32_t permissions,
        const char *type, uint32_t version, const struct spa_dict *props)
{
    struct pw_graph_shim *shim = data;
    const char *name;
    const char *media_type;
    const char *direction;
    (void)permissions;
    (void)version;

    if (type == NULL) {
        return;
    }
    if (strcmp(type, PW_TYPE_INTERFACE_Node) == 0) {
        int index = node_index(shim, id);
        if (index < 0 && shim->node_count < PW_GRAPH_MAX_NODES) {
            index = (int)shim->node_count++;
            shim->nodes[index].id = id;
        }
        if (index >= 0) {
            name = props == NULL ? NULL : spa_dict_lookup(props, PW_KEY_NODE_NAME);
            if (name == NULL && props != NULL) {
                name = spa_dict_lookup(props, PW_KEY_NODE_DESCRIPTION);
            }
            copy_text(shim->nodes[index].name, sizeof(shim->nodes[index].name), name, "PipeWire node");
        }
    } else if (strcmp(type, PW_TYPE_INTERFACE_Port) == 0) {
        int index = port_index(shim, id);
        if (index < 0 && shim->port_count < PW_GRAPH_MAX_PORTS) {
            index = (int)shim->port_count++;
            shim->ports[index].id = id;
        }
        if (index >= 0 && props != NULL) {
            shim->ports[index].node_id = parse_u32(spa_dict_lookup(props, PW_KEY_NODE_ID), 0);
            name = spa_dict_lookup(props, PW_KEY_PORT_NAME);
            copy_text(shim->ports[index].name, sizeof(shim->ports[index].name), name, "PipeWire port");
            direction = spa_dict_lookup(props, PW_KEY_PORT_DIRECTION);
            shim->ports[index].direction = direction != NULL && strcmp(direction, "out") == 0 ? 1 : 0;
            media_type = spa_dict_lookup(props, PW_KEY_MEDIA_TYPE);
            copy_text(shim->ports[index].media_type, sizeof(shim->ports[index].media_type), media_type, "Unknown");
        }
    } else if (strcmp(type, PW_TYPE_INTERFACE_Link) == 0) {
        int index = link_index(shim, id);
        if (index < 0 && shim->link_count < PW_GRAPH_MAX_LINKS) {
            index = (int)shim->link_count++;
            shim->links[index].id = id;
        }
        if (index >= 0 && props != NULL) {
            shim->links[index].output_port = parse_u32(spa_dict_lookup(props, PW_KEY_LINK_OUTPUT_PORT), 0);
            shim->links[index].input_port = parse_u32(spa_dict_lookup(props, PW_KEY_LINK_INPUT_PORT), 0);
        }
    }
}

static void on_registry_global_remove(void *data, uint32_t id)
{
    struct pw_graph_shim *shim = data;
    remove_node(shim, id);
    remove_port(shim, id);
    remove_link(shim, id);
}

static void on_core_done(void *data, uint32_t id, int seq)
{
    struct pw_graph_shim *shim = data;
    if (id == PW_ID_CORE && seq == shim->sync_seq) {
        shim->sync_done = true;
        pw_thread_loop_signal(shim->thread_loop, false);
    }
}

static void on_core_error(void *data, uint32_t id, int seq, int res, const char *message)
{
    struct pw_graph_shim *shim = data;
    (void)id;
    (void)seq;
    (void)message;
    shim->sync_error = res == 0 ? -EIO : res;
    pw_thread_loop_signal(shim->thread_loop, false);
}

static const struct pw_core_events core_events = {
    .version = PW_VERSION_CORE_EVENTS,
    .done = on_core_done,
    .error = on_core_error,
};

static const struct pw_registry_events registry_events = {
    .version = PW_VERSION_REGISTRY_EVENTS,
    .global = on_registry_global,
    .global_remove = on_registry_global_remove,
};

static int do_sync(struct pw_graph_shim *shim)
{
    shim->sync_error = 0;
    shim->sync_done = false;
    shim->sync_seq = pw_core_sync(shim->core, PW_ID_CORE, 0);
    if (shim->sync_seq < 0) {
        return shim->sync_seq;
    }
    while (!shim->sync_done && shim->sync_error == 0) {
        pw_thread_loop_wait(shim->thread_loop);
    }
    return shim->sync_error;
}

struct pw_graph_shim *pw_graph_shim_new(void)
{
    struct pw_graph_shim *shim = calloc(1, sizeof(*shim));
    if (shim == NULL) {
        return NULL;
    }
    pw_init(NULL, NULL);
    shim->thread_loop = pw_thread_loop_new("qpwgraph-rs", NULL);
    if (shim->thread_loop == NULL) {
        free(shim);
        return NULL;
    }
    shim->context = pw_context_new(pw_thread_loop_get_loop(shim->thread_loop), NULL, 0);
    if (shim->context == NULL) {
        pw_thread_loop_destroy(shim->thread_loop);
        free(shim);
        return NULL;
    }
    shim->core = pw_context_connect(shim->context, NULL, 0);
    if (shim->core == NULL) {
        pw_context_destroy(shim->context);
        pw_thread_loop_destroy(shim->thread_loop);
        free(shim);
        return NULL;
    }
    shim->registry = pw_core_get_registry(shim->core, PW_VERSION_REGISTRY, 0);
    if (shim->registry == NULL ||
            pw_core_add_listener(shim->core, &shim->core_listener, &core_events, shim) < 0 ||
            pw_registry_add_listener(shim->registry, &shim->registry_listener, &registry_events, shim) < 0 ||
            pw_thread_loop_start(shim->thread_loop) < 0) {
        pw_core_disconnect(shim->core);
        pw_context_destroy(shim->context);
        pw_thread_loop_destroy(shim->thread_loop);
        free(shim);
        return NULL;
    }
    return shim;
}

void pw_graph_shim_free(struct pw_graph_shim *shim)
{
    if (shim == NULL) {
        return;
    }
    pw_thread_loop_stop(shim->thread_loop);
    spa_hook_remove(&shim->registry_listener);
    spa_hook_remove(&shim->core_listener);
    pw_core_disconnect(shim->core);
    pw_context_destroy(shim->context);
    pw_thread_loop_destroy(shim->thread_loop);
    free(shim);
}

int pw_graph_shim_snapshot(struct pw_graph_shim *shim, struct pw_graph_snapshot *snapshot)
{
    size_t i;
    if (shim == NULL || snapshot == NULL) {
        return -EINVAL;
    }
    pw_thread_loop_lock(shim->thread_loop);
    int result = do_sync(shim);
    if (result == 0) {
        memset(snapshot, 0, sizeof(*snapshot));
        snapshot->node_count = (uint32_t)shim->node_count;
        snapshot->port_count = (uint32_t)shim->port_count;
        snapshot->link_count = (uint32_t)shim->link_count;
        for (i = 0; i < shim->node_count; ++i) snapshot->nodes[i] = shim->nodes[i];
        for (i = 0; i < shim->port_count; ++i) snapshot->ports[i] = shim->ports[i];
        for (i = 0; i < shim->link_count; ++i) snapshot->links[i] = shim->links[i];
    }
    pw_thread_loop_unlock(shim->thread_loop);
    return result;
}

int pw_graph_shim_create_link(struct pw_graph_shim *shim,
        uint32_t output_node, uint32_t output_port,
        uint32_t input_node, uint32_t input_port, uint32_t *link_id)
{
    struct pw_properties *properties;
    struct pw_proxy *proxy;
    int result;
    if (shim == NULL || link_id == NULL) {
        return -EINVAL;
    }
    pw_thread_loop_lock(shim->thread_loop);
    properties = pw_properties_new(NULL, NULL);
    if (properties == NULL) {
        pw_thread_loop_unlock(shim->thread_loop);
        return -ENOMEM;
    }
    pw_properties_setf(properties, PW_KEY_LINK_OUTPUT_NODE, "%u", output_node);
    pw_properties_setf(properties, PW_KEY_LINK_OUTPUT_PORT, "%u", output_port);
    pw_properties_setf(properties, PW_KEY_LINK_INPUT_NODE, "%u", input_node);
    pw_properties_setf(properties, PW_KEY_LINK_INPUT_PORT, "%u", input_port);
    proxy = (struct pw_proxy *)pw_core_create_object(
        shim->core, "link-factory", PW_TYPE_INTERFACE_Link, PW_VERSION_LINK,
        &properties->dict, 0);
    pw_properties_free(properties);
    if (proxy == NULL) {
        int error = errno;
        pw_thread_loop_unlock(shim->thread_loop);
        return error == 0 ? -EIO : -error;
    }
    result = do_sync(shim);
    if (result == 0) {
        *link_id = pw_proxy_get_bound_id(proxy);
        if (*link_id == SPA_ID_INVALID) {
            result = -ENOENT;
        }
    }
    pw_thread_loop_unlock(shim->thread_loop);
    return result;
}

int pw_graph_shim_destroy_link(struct pw_graph_shim *shim, uint32_t link_id)
{
    int result;
    if (shim == NULL) {
        return -EINVAL;
    }
    pw_thread_loop_lock(shim->thread_loop);
    result = pw_registry_destroy(shim->registry, link_id);
    if (result >= 0) {
        result = do_sync(shim);
    }
    pw_thread_loop_unlock(shim->thread_loop);
    return result;
}
