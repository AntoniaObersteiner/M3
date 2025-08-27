/*
 * Copyright (C) 2018 Nils Asmussen <nils@os.inf.tu-dresden.de>
 * Economic rights: Technische Universitaet Dresden (Germany)
 *
 * Copyright (C) 2019-2020 Nils Asmussen, Barkhausen Institut
 *
 * This file is part of M3 (Microkernel-based SysteM for Heterogeneous Manycores).
 *
 * M3 is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License version 2 as
 * published by the Free Software Foundation.
 *
 * M3 is distributed in the hope that it will be useful, but
 * WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU
 * General Public License version 2 for more details.
 */

#include <m3/Syscalls.h>
#include <m3/com/RecvGate.h>
#include <m3/stream/Standard.h>

using namespace m3;

const char * name = "dosa"; // may be set by main

void create_and_destroy (size_t send_gate_count, size_t print_interval) {
	constexpr auto gate_size = 512;
	constexpr auto mesg_size = 64;
	constexpr auto gate_order = nextlog2<gate_size>::val;
	constexpr auto mesg_order = nextlog2<mesg_size>::val;
	println("{}: creating receive gate ({}, {})"_cf, name, gate_order, mesg_order);
    RecvGate rgate = RecvGate::create(gate_order, mesg_order);

    capsel_t const first_sel = 1000;
    for (size_t i = 0; i < send_gate_count; i++) {
		capsel_t sel = first_sel + i;
		if (i % print_interval == 0) println(
			"{}: creating {}'th sending gate (send sel: {}, recv sel: {}, 0, UNLIMITED)"_cf,
			name, sel - first_sel, sel, rgate.sel()
		);
        try {
            m3::Syscalls::create_sgate(sel, rgate.sel(), 0, SendGate::UNLIMITED);
        }
        catch(const Exception &e) {
            eprintln("{}: Unable to create sgate: {}"_cf, name, e.what());
        }
    }

	println("{}: deleting all those endpoints..."_cf, name);
}

int main(int argc, char ** argv) {
	if (argc > 1) {
		name = argv[1];
	}
	println("Hi, it's {}!"_cf, name);

	constexpr size_t send_gate_count = 1000;
	constexpr size_t print_interval = 100;
	while (1) {
		create_and_destroy(send_gate_count, print_interval);
	}

    return 0;
}
