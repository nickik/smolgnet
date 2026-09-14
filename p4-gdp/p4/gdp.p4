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

/*
 * x4c's Rust target currently has little coverage for 64-bit scalar fields.
 * Keep the exact 128-bit wire layout while representing each 64-bit GDP
 * address as two 32-bit words. The logical address is destination_hi/lo or
 * source_hi/lo; this is an implementation detail, not a GDP wire change.
 */
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

    /* Local delivery does not consume a routing hop. */
    action deliver_local(bit<16> port) {
        egress.port = port;
        egress.transit = false;
    }

    /* Transit forwarding is followed by common hop-limit processing. */
    action forward(bit<16> port) {
        egress.port = port;
        egress.transit = true;
    }

    table global_routes {
        key = {
            hdr.global_addr.destination_hi: exact;
            hdr.global_addr.destination_lo: exact;
        }
        actions = {
            drop;
            deliver_local;
            forward;
        }
        default_action = drop;
        size = 1024;
    }

    table local_routes {
        key = {
            hdr.local_addr.destination: exact;
        }
        actions = {
            drop;
            deliver_local;
            forward;
        }
        default_action = drop;
        size = 1024;
    }

    apply {
        if (hdr.global_addr.isValid()) {
            global_routes.apply();
        } else {
            local_routes.apply();
        }

        /*
         * GDP CRC-8 deliberately excludes hop limit, so a router can update
         * hop without recalculating the CRC. Only transit packets consume a
         * hop. A transit packet with hop 0 or 1 expires at this router.
         */
        if (egress.transit) {
            if (hdr.base.hop == 8w0) {
                egress.drop = true;
            } else {
                if (hdr.base.hop == 8w1) {
                    egress.drop = true;
                } else {
                    hdr.base.hop = hdr.base.hop - 8w1;
                }
            }
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
