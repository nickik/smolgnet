#include <core.p4>

struct ingress_metadata_t { bit<16> port; bool drop; }
struct egress_metadata_t { bit<16> port; bool drop; bool broadcast; bool transit; }

#include "../../p4-common/gdp_wire.p4"

control ingress(inout headers_t hdr, inout ingress_metadata_t ingress, inout egress_metadata_t egress) {
    action drop() { egress.drop = true; }
    action forward(bit<16> port) { egress.port = port; egress.transit = false; }
    table global_nodes {
        key = { hdr.global_addr.destination_hi: exact; hdr.global_addr.destination_lo: exact; }
        actions = { drop; forward; }
        default_action = drop; size = 4096;
    }
    apply {
        egress.transit = false;
        if (hdr.global_addr.isValid()) { global_nodes.apply(); }
        else { egress.drop = true; }
    }
}
control egress(inout headers_t hdr, inout ingress_metadata_t ingress, inout egress_metadata_t egress) { }
SoftNPU(parse(), ingress(), egress()) main;
