#include <core.p4>

struct ingress_metadata_t { bit<16> port; bool drop; }
struct egress_metadata_t { bit<16> port; bool drop; bool broadcast; bool transit; bool decided; }

#include "../../p4-common/gdp_wire.p4"

control ingress(inout headers_t hdr, inout ingress_metadata_t ingress, inout egress_metadata_t egress) {
    action allow() { }
    action drop() { egress.drop = true; egress.decided = true; egress.transit = false; }
    action punt(bit<16> cpu_port) { egress.port = cpu_port; egress.decided = true; egress.transit = false; }
    action forward(bit<16> port) { egress.port = port; egress.decided = true; egress.transit = true; }
    table global_long_routes {
        key = { hdr.global_addr.destination_hi: exact; hdr.global_addr.destination_lo: lpm; }
        actions = { allow; drop; punt; forward; }
        default_action = allow; size = 4096;
    }
    table global_short_routes {
        key = { hdr.global_addr.destination_hi: lpm; }
        actions = { drop; punt; forward; }
        default_action = drop; size = 4096;
    }
    table local_routes {
        key = { ingress.port: exact; hdr.local_addr.destination: exact; }
        actions = { drop; punt; }
        default_action = drop; size = 1024;
    }
    apply {
        egress.decided = false; egress.transit = false;
        if (hdr.global_addr.isValid()) {
            global_long_routes.apply();
            if (egress.decided == false) { global_short_routes.apply(); }
        } else { local_routes.apply(); }
        if (egress.transit) {
            if (hdr.base.hop == 8w0) { egress.drop = true; }
            else { if (hdr.base.hop == 8w1) { egress.drop = true; } else { hdr.base.hop = hdr.base.hop - 8w1; } }
        }
    }
}
control egress(inout headers_t hdr, inout ingress_metadata_t ingress, inout egress_metadata_t egress) { }
SoftNPU(parse(), ingress(), egress()) main;
