#include <core.p4>

struct ingress_metadata_t {
    bit<16> port;
    bool drop;
}

struct egress_metadata_t {
    bit<16> port;
    bool drop;
    bool broadcast;
    bool transit;
}

header gdp_base_h {
    bit<2> version;
    bit<4> packet_type;
    bit<4> size_class;
    bit<1> local_form;
    bit<5> reserved;
    bit<8> crc8;
    bit<8> hop;
}

/* Keep the same GDP parser/layout used by smolgnet's existing P4 backend. */
header gdp_global_h {
    bit<32> destination_hi;
    bit<32> destination_lo;
    bit<32> source_hi;
    bit<32> source_lo;
}

header gdp_local_h {
    bit<16> destination;
    bit<16> source;
}

struct headers_t {
    gdp_base_h base;
    gdp_global_h global_addr;
    gdp_local_h local_addr;
}

parser parse(
    packet_in pkt,
    out headers_t hdr,
    inout ingress_metadata_t ingress,
) {
    state start {
        pkt.extract(hdr.base);
        if (hdr.base.local_form == 1w1) {
            transition parse_local;
        }
        transition parse_global;
    }

    state parse_global {
        pkt.extract(hdr.global_addr);
        transition accept;
    }

    state parse_local {
        pkt.extract(hdr.local_addr);
        transition accept;
    }
}

control ingress(
    inout headers_t hdr,
    inout ingress_metadata_t ingress,
    inout egress_metadata_t egress,
) {
    action drop() {
        egress.drop = true;
    }

    action forward(bit<16> port) {
        egress.port = port;
        egress.transit = false;
    }

    table global_nodes {
        key = {
            hdr.global_addr.destination_hi: exact;
            hdr.global_addr.destination_lo: exact;
        }
        actions = {
            drop;
            forward;
        }
        default_action = drop;
        size = 4096;
    }

    apply {
        egress.transit = false;
        if (hdr.global_addr.isValid()) {
            global_nodes.apply();
        } else {
            /* Local-form GDP is link-local and never switched as node traffic. */
            egress.drop = true;
        }
    }
}

control egress(
    inout headers_t hdr,
    inout ingress_metadata_t ingress,
    inout egress_metadata_t egress,
) {
}

SoftNPU(
    parse(),
    ingress(),
    egress()
) main;
