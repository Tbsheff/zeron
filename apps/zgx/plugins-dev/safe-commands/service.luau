local zeron = require("./zeron")("safe-commands")

local ALLOWED = { "ls", "cat", "rg", "git status", "git diff" }
local REFUSED = { "rm%s+%-%a*[rR]%a*[fF]%a*%s+/", "rm%s+%-%a*[fF]%a*[rR]%a*%s+/", "mkfs", "dd%s+if=.-of=/dev/" }
local CHAINS = "[;&|`$<>\n\r]"
local RUNS_OR_WRITES = { "%-%-pre", "%-%-output", "%-%-ext%-diff" }

local function allowed(command: string): boolean
	if string.find(command, CHAINS) then
		return false
	end
	for _, flag in RUNS_OR_WRITES do
		if string.find(command, flag) then
			return false
		end
	end
	for _, prefix in ALLOWED do
		if command == prefix or string.sub(command, 1, #prefix + 1) == prefix .. " " then
			return true
		end
	end
	return false
end

local function refused(command: string): boolean
	for _, pattern in REFUSED do
		if string.find(command, pattern) then
			return true
		end
	end
	return false
end

zeron.hook("tool.approve", function(ctx: any)
	local command = ctx.kind == "command" and ctx.input and ctx.input.command
	if type(command) ~= "string" then
		return "ask"
	end
	command = string.gsub(command, "^%s+", "")
	if refused(command) then
		return "deny"
	elseif allowed(command) then
		return "allow"
	end
	return "ask"
end)
