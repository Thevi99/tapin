#!/bin/sh
# A stand-in agent screen for the README demo (plain text, no real agent).
printf '\033[2J\033[H'
printf '\033[1mshop-api\033[0m  ·  agent session\n\n'
printf '\033[36m›\033[0m fix the flaky checkout test\n\n'
printf '  • Edit src/checkout_test.go\n'
printf '  • Run  go test ./checkout/... -count=5   \033[32m✓ approved in tapin\033[0m\n'
printf '      ok  shop/checkout  1.84s  (5 runs)\n\n'
printf 'Fixed: the test raced on the cart clock. 5/5 runs pass.\n\n'
printf '\033[36m›\033[0m '
sleep 600
